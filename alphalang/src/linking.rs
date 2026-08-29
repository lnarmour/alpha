use std::fmt::{self, Display};

use hugr::builder::{Dataflow, DataflowSubContainer, HugrBuilder, ModuleBuilder};
use hugr::core::Visibility;
use hugr::ops::OpType;
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

#[cfg(test)]
mod tests {
    use hugr::builder::{
        Container, DFGBuilder, Dataflow, DataflowHugr, HugrBuilder, ModuleBuilder,
    };
    use hugr::core::Visibility;
    use hugr::extension::prelude::bool_t;
    use hugr::metadata::Metadata;
    use hugr::ops::OpType;
    use hugr::types::Signature;
    use hugr::{Hugr, HugrView};

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
}