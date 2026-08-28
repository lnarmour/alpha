#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Multiplicity {
    Linear,
    #[default]
    Unrestricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VariableId(u32);

impl VariableId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index.try_into().expect("too many variables in one system"))
    }
}