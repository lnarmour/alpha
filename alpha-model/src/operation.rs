use crate::{ElementType, Multiplicity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegisteredOperation {
    QAlloc,
    H,
    Cx,
    Measure,
    Discard,
}

impl RegisteredOperation {
    pub fn name(self) -> &'static str {
        match self {
            Self::QAlloc => "qalloc",
            Self::H => "h",
            Self::Cx => "cx",
            Self::Measure => "measure",
            Self::Discard => "discard",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub element_type: ElementType,
    pub multiplicity: Multiplicity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Continuity {
    pub input: usize,
    pub output: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSignature {
    pub operation: RegisteredOperation,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub continuity: Vec<Continuity>,
}

fn port(element_type: ElementType, multiplicity: Multiplicity) -> Port {
    Port {
        element_type,
        multiplicity,
    }
}

pub fn registered_operation(name: &str) -> Option<OperationSignature> {
    use ElementType::{Bool, Qubit};
    use Multiplicity::{Linear, Unrestricted};
    use RegisteredOperation::{Cx, Discard, Measure, QAlloc, H};

    let signature = match name {
        "qalloc" => OperationSignature {
            operation: QAlloc,
            inputs: vec![],
            outputs: vec![port(Qubit, Linear)],
            continuity: vec![],
        },
        "h" => OperationSignature {
            operation: H,
            inputs: vec![port(Qubit, Linear)],
            outputs: vec![port(Qubit, Linear)],
            continuity: vec![Continuity {
                input: 0,
                output: 0,
            }],
        },
        "cx" => OperationSignature {
            operation: Cx,
            inputs: vec![port(Qubit, Linear), port(Qubit, Linear)],
            outputs: vec![port(Qubit, Linear), port(Qubit, Linear)],
            continuity: vec![
                Continuity {
                    input: 0,
                    output: 0,
                },
                Continuity {
                    input: 1,
                    output: 1,
                },
            ],
        },
        "measure" => OperationSignature {
            operation: Measure,
            inputs: vec![port(Qubit, Linear)],
            outputs: vec![port(Bool, Unrestricted)],
            continuity: vec![],
        },
        "discard" => OperationSignature {
            operation: Discard,
            inputs: vec![port(Qubit, Linear)],
            outputs: vec![],
            continuity: vec![],
        },
        _ => return None,
    };
    Some(signature)
}
