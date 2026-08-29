use std::fmt::{self, Display};

use hugr::builder::{Dataflow, DataflowSubContainer, HugrBuilder, ModuleBuilder};
use hugr::core::Visibility;
use hugr::envelope::EnvelopeConfig;
use hugr::hugr::linking::{HugrLinking, NameLinkingPolicy, OnMultiDefn, OnNewFunc};
use hugr::ops::OpType;
use hugr::package::Package;
use hugr::types::PolyFuncType;
use hugr::{Hugr, HugrView};

#[derive(Debug)]
pub(crate) struct LinkError(String);

impl LinkError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for LinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LinkError {}

fn implementation_module(implementation: Hugr, symbol: &str) -> Result<Hugr, LinkError> {
    if symbol.trim().is_empty() {
        return Err(LinkError::new("link symbol must not be empty"));
    }
    implementation
        .validate()
        .map_err(|error| LinkError::new(format!("invalid Alpha HUGR: {error}")))?;

    let signature = match implementation.entrypoint_optype() {
        OpType::DFG(dfg) => dfg.signature.clone(),
        _ => return Err(LinkError::new("Alpha HUGR entry point must be a DFG")),
    };
    let output_count = signature.output_count();
    let mut module = ModuleBuilder::new();
    let mut function = module
        .define_function_vis(symbol, signature, Visibility::Public)
        .map_err(|error| LinkError::new(format!("cannot create Alpha function: {error}")))?;
    let inputs = function.input_wires().collect::<Vec<_>>();
    let nested = function
        .add_hugr_with_wires(implementation, inputs)
        .map_err(|error| LinkError::new(format!("cannot embed Alpha DFG: {error}")))?;
    function
        .finish_with_outputs((0..output_count).map(|port| nested.out_wire(port)))
        .map_err(|error| LinkError::new(format!("cannot finish Alpha function: {error}")))?;
    module
        .finish_hugr()
        .map_err(|error| LinkError::new(format!("invalid Alpha function module: {error}")))
}

struct Target {
    signature: PolyFuncType,
}

fn find_target(module: &Hugr, symbol: &str) -> Result<Target, LinkError> {
    let mut public = Vec::new();
    let mut has_private = false;
    for node in module.children(module.module_root()) {
        let Some((name, visibility, signature)) = (match module.get_optype(node) {
            OpType::FuncDecl(function) => Some((
                function.func_name(),
                function.visibility(),
                function.signature(),
            )),
            OpType::FuncDefn(function) => Some((
                function.func_name(),
                function.visibility(),
                function.signature(),
            )),
            _ => None,
        }) else {
            continue;
        };
        if name != symbol {
            continue;
        }
        if visibility == &Visibility::Public {
            public.push(Target {
                signature: signature.clone(),
            });
        } else {
            has_private = true;
        }
    }

    let target = match public.len() {
        0 if has_private => {
            return Err(LinkError::new(format!("symbol '{symbol}' is private")));
        }
        0 => {
            return Err(LinkError::new(format!(
                "public symbol '{symbol}' was not found"
            )));
        }
        1 => public.pop().unwrap(),
        _ => {
            return Err(LinkError::new(format!(
                "more than one public symbol '{symbol}' was found"
            )));
        }
    };
    if !target.signature.params().is_empty() {
        return Err(LinkError::new(format!(
            "symbol '{symbol}' is polymorphic"
        )));
    }
    Ok(target)
}

pub(crate) fn link_alpha_function_bytes(
    wrapper: &[u8],
    implementation: &str,
    symbol: &str,
) -> Result<Vec<u8>, LinkError> {
    if symbol.trim().is_empty() {
        return Err(LinkError::new("link symbol must not be empty"));
    }
    let mut package = Package::load(wrapper, None)
        .map_err(|error| LinkError::new(format!("invalid wrapper package: {error}")))?;
    if package.modules.len() != 1 {
        return Err(LinkError::new(format!(
            "wrapper package must contain exactly one module, found {}",
            package.modules.len()
        )));
    }
    package
        .validate()
        .map_err(|error| LinkError::new(format!("invalid wrapper package: {error}")))?;

    let (alpha, alpha_extensions) = Hugr::load_with_exts(implementation.as_bytes(), None)
        .map_err(|error| LinkError::new(format!("invalid Alpha HUGR: {error}")))?;
    let target = find_target(&package.modules[0], symbol)?;
    let alpha_signature = match alpha.entrypoint_optype() {
        OpType::DFG(dfg) => dfg.signature.clone(),
        _ => return Err(LinkError::new("Alpha HUGR entry point must be a DFG")),
    };
    if target.signature.body() != &alpha_signature {
        return Err(LinkError::new(format!(
            "signature mismatch for '{symbol}': wrapper has {}, Alpha has {}",
            target.signature.body(),
            alpha_signature
        )));
    }

    package.extensions.extend(&alpha_extensions);
    let implementation = implementation_module(alpha, symbol)?;
    let mut wrapper_module = package.modules.pop().unwrap();
    let old_entrypoint = wrapper_module.entrypoint();
    let policy = NameLinkingPolicy::new_keep_both_invalid()
        .on_new_names(OnNewFunc::RaiseError)
        .on_signature_conflict(OnNewFunc::RaiseError)
        .on_multiple_defn(OnMultiDefn::UseSource);
    wrapper_module
        .link_module(implementation, &policy)
        .map_err(|error| LinkError::new(format!("cannot link '{symbol}': {error}")))?;
    if wrapper_module.entrypoint() != old_entrypoint {
        return Err(LinkError::new(
            "HUGR linker changed the wrapper entry point",
        ));
    }

    package.modules.push(wrapper_module);
    package
        .validate()
        .map_err(|error| LinkError::new(format!("invalid linked package: {error}")))?;
    let mut bytes = Vec::new();
    package
        .store(&mut bytes, EnvelopeConfig::binary())
        .map_err(|error| LinkError::new(format!("cannot serialize linked package: {error}")))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use hugr::builder::{
        Container, DFGBuilder, Dataflow, DataflowHugr, HugrBuilder, ModuleBuilder,
    };
    use hugr::core::Visibility;
    use hugr::envelope::EnvelopeConfig;
    use hugr::extension::prelude::{bool_t, usize_t};
    use hugr::hugr::hugrmut::HugrMut;
    use hugr::metadata::Metadata;
    use hugr::ops::handle::NodeHandle;
    use hugr::ops::{FuncDecl, OpType};
    use hugr::package::Package;
    use hugr::types::type_param::TypeParam;
    use hugr::types::{PolyFuncType, Signature};
    use hugr::{Hugr, HugrView, Node};

    use super::*;

    struct Marker;

    impl Metadata for Marker {
        type Type<'hugr> = &'hugr str;
        const KEY: &'static str = "alpha.test.marker";
    }

    fn identity_dfg() -> Hugr {
        let mut builder = DFGBuilder::new(Signature::new_endo([bool_t()])).unwrap();
        let input = builder.input_wires().next().unwrap();
        builder.set_metadata::<Marker>("preserved");
        builder.finish_hugr_with_outputs([input]).unwrap()
    }

    fn identity_dfg_text() -> String {
        identity_dfg()
            .store_str(EnvelopeConfig::text())
            .unwrap()
    }

    fn package_bytes(package: &Package) -> Vec<u8> {
        let mut bytes = Vec::new();
        package
            .store(&mut bytes, EnvelopeConfig::binary())
            .unwrap();
        bytes
    }

    fn wrapper_with_declaration() -> Package {
        let signature = Signature::new_endo([bool_t()]);
        let mut module = ModuleBuilder::new();
        let target = module.declare("foo", signature.clone().into()).unwrap();
        let mut main = module.define_function("main", signature).unwrap();
        let call = main.call(&target, &[], main.input_wires()).unwrap();
        let main = main.finish_with_outputs(call.outputs()).unwrap();
        let mut module = module.finish_hugr().unwrap();
        module.set_entrypoint(main.node());
        Package::from_hugr(module)
    }

    fn wrapper_with_dummy_definition() -> Package {
        let signature = Signature::new_endo([bool_t()]);
        let mut module = ModuleBuilder::new();
        let dummy = module
            .define_function_vis("foo", signature.clone(), Visibility::Public)
            .unwrap();
        let input = dummy.input_wires().next().unwrap();
        let dummy = dummy.finish_with_outputs([input]).unwrap();
        let mut main = module.define_function("main", signature).unwrap();
        let call = main.call(dummy.handle(), &[], main.input_wires()).unwrap();
        let main = main.finish_with_outputs(call.outputs()).unwrap();
        let mut module = module.finish_hugr().unwrap();
        module.set_entrypoint(main.node());
        Package::from_hugr(module)
    }

    fn public_functions(module: &Hugr, symbol: &str) -> Vec<Node> {
        module
            .children(module.module_root())
            .filter(|node| match module.get_optype(*node) {
                OpType::FuncDecl(function) => {
                    function.func_name() == symbol
                        && function.visibility() == &Visibility::Public
                }
                OpType::FuncDefn(function) => {
                    function.func_name() == symbol
                        && function.visibility() == &Visibility::Public
                }
                _ => false,
            })
            .collect()
    }

    fn assert_link_error(wrapper: Package, symbol: &str, expected: &str) {
        let error = link_alpha_function_bytes(
            &package_bytes(&wrapper),
            &identity_dfg_text(),
            symbol,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
    }

    fn wrapper_with_private_foo() -> Package {
        let mut module = ModuleBuilder::new();
        let function = module
            .define_function("foo", Signature::new_endo([bool_t()]))
            .unwrap();
        let input = function.input_wires().next().unwrap();
        function.finish_with_outputs([input]).unwrap();
        Package::from_hugr(module.finish_hugr().unwrap())
    }

    fn wrapper_with_polymorphic_foo() -> Package {
        let mut module = ModuleBuilder::new();
        module
            .declare(
                "foo",
                PolyFuncType::new(
                    [TypeParam::max_nat_kind()],
                    Signature::new_endo([bool_t()]),
                ),
            )
            .unwrap();
        Package::from_hugr(module.finish_hugr().unwrap())
    }

    fn wrapper_with_wrong_signature() -> Package {
        let mut module = ModuleBuilder::new();
        module
            .declare("foo", Signature::new_endo([usize_t()]).into())
            .unwrap();
        Package::from_hugr(module.finish_hugr().unwrap())
    }

    fn wrapper_with_duplicate_foo() -> Package {
        let mut package = wrapper_with_declaration();
        let module = &mut package.modules[0];
        module.add_node_with_parent(
            module.module_root(),
            FuncDecl::new("foo", Signature::new_endo([bool_t()])),
        );
        package
    }

    #[test]
    fn promotes_dfg_to_public_named_function() {
        let module = implementation_module(identity_dfg(), "foo").unwrap();
        module.validate().unwrap();

        let functions = module
            .children(module.module_root())
            .filter_map(|node| match module.get_optype(node) {
                OpType::FuncDefn(definition) => Some((node, definition)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].1.func_name(), "foo");
        assert_eq!(functions[0].1.visibility(), &Visibility::Public);
        assert!(functions[0].1.signature().params().is_empty());
        assert_eq!(
            functions[0].1.signature().body(),
            &Signature::new_endo([bool_t()])
        );

        let nested_dfg = module
            .children(functions[0].0)
            .find(|node| matches!(module.get_optype(*node), OpType::DFG(_)))
            .unwrap();
        assert_eq!(
            module.get_metadata::<Marker>(nested_dfg),
            Some("preserved")
        );
    }

    #[test]
    fn rejects_empty_symbol_and_non_dfg_entrypoint() {
        assert!(
            implementation_module(identity_dfg(), "   ")
                .unwrap_err()
                .to_string()
                .contains("symbol must not be empty")
        );
        assert!(
            implementation_module(ModuleBuilder::new().finish_hugr().unwrap(), "foo")
                .unwrap_err()
                .to_string()
                .contains("entry point must be a DFG")
        );
    }

    #[test]
    fn replaces_declaration_and_preserves_entrypoint() {
        let wrapper = wrapper_with_declaration();
        let linked = link_alpha_function_bytes(
            &package_bytes(&wrapper),
            &identity_dfg_text(),
            "foo",
        )
        .unwrap();
        let linked = Package::load(linked.as_slice(), None).unwrap();
        linked.validate().unwrap();

        let module = &linked.modules[0];
        assert!(matches!(
            module.get_optype(module.entrypoint()),
            OpType::FuncDefn(function) if function.func_name() == "main"
        ));
        let foo = public_functions(module, "foo");
        assert_eq!(foo.len(), 1);
        assert!(matches!(module.get_optype(foo[0]), OpType::FuncDefn(_)));
        let call = module
            .nodes()
            .find(|node| matches!(module.get_optype(*node), OpType::Call(_)))
            .unwrap();
        assert_eq!(module.static_source(call), Some(foo[0]));
    }

    #[test]
    fn replaces_dummy_definition() {
        let wrapper = wrapper_with_dummy_definition();
        let linked = link_alpha_function_bytes(
            &package_bytes(&wrapper),
            &identity_dfg_text(),
            "foo",
        )
        .unwrap();
        let linked = Package::load(linked.as_slice(), None).unwrap();
        let module = &linked.modules[0];

        assert_eq!(public_functions(module, "foo").len(), 1);
        assert_eq!(
            module
                .nodes()
                .filter(|node| module.get_optype(*node).is_dfg())
                .count(),
            1
        );
        let nested_dfg = module
            .nodes()
            .find(|node| module.get_optype(*node).is_dfg())
            .unwrap();
        assert_eq!(
            module.get_metadata::<Marker>(nested_dfg),
            Some("preserved")
        );
    }

    #[test]
    fn rejects_missing_duplicate_private_and_polymorphic_targets() {
        assert_link_error(
            Package::from_hugr(ModuleBuilder::new().finish_hugr().unwrap()),
            "foo",
            "public symbol 'foo' was not found",
        );
        assert_link_error(
            wrapper_with_duplicate_foo(),
            "foo",
            "more than one public symbol 'foo'",
        );
        assert_link_error(
            wrapper_with_private_foo(),
            "foo",
            "symbol 'foo' is private",
        );
        assert_link_error(
            wrapper_with_polymorphic_foo(),
            "foo",
            "symbol 'foo' is polymorphic",
        );
    }

    #[test]
    fn rejects_signature_mismatch_and_multiple_modules() {
        assert_link_error(
            wrapper_with_wrong_signature(),
            "foo",
            "signature mismatch for 'foo'",
        );
        assert_link_error(
            Package::new([
                ModuleBuilder::new().finish_hugr().unwrap(),
                ModuleBuilder::new().finish_hugr().unwrap(),
            ]),
            "foo",
            "exactly one module",
        );
    }

    #[test]
    fn rejects_malformed_envelopes() {
        let wrapper_error =
            link_alpha_function_bytes(b"not a package", &identity_dfg_text(), "foo")
                .unwrap_err();
        assert!(wrapper_error.to_string().contains("invalid wrapper package"));

        let alpha_error = link_alpha_function_bytes(
            &package_bytes(&wrapper_with_declaration()),
            "not a HUGR",
            "foo",
        )
        .unwrap_err();
        assert!(alpha_error.to_string().contains("invalid Alpha HUGR"));
    }
}