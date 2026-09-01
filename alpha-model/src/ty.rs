#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ElementType {
    #[default]
    Unspecified,
    Bool,
    Int,
    Real,
    Qubit,
}

impl From<alpha_syntax::ast::ElementType> for ElementType {
    fn from(element_type: alpha_syntax::ast::ElementType) -> Self {
        match element_type {
            alpha_syntax::ast::ElementType::Bool => Self::Bool,
            alpha_syntax::ast::ElementType::Int => Self::Int,
            alpha_syntax::ast::ElementType::Real => Self::Real,
            alpha_syntax::ast::ElementType::Qubit => Self::Qubit,
        }
    }
}
