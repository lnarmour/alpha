use std::fs;
use std::process::Command;

const QUANTUM_CHAIN: &str = include_str!("../../alpha-codegen/tests/src/quantum_chain.alpha");
const QUANTUM_CX: &str = include_str!("../../alpha-codegen/tests/src/quantum_cx.alpha");
const SCHEDULE: &str =
    "[T,N] -> { Q__call0[t,i] -> [t,0,i]; Q__call1[t,i] -> [t,1,i]; M__call0[i] -> [T,2,i] }";

fn temporary_file(name: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("alphac-{}-{name}", std::process::id()));
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn emits_scheduled_hugr() {
    let source = temporary_file("quantum.alpha", QUANTUM_CHAIN);
    let schedule = temporary_file("quantum.isl", SCHEDULE);
    let output = Command::new(env!("CARGO_BIN_EXE_alphac"))
        .args([
            "--emit",
            "hugr",
            "--schedule",
            schedule.to_str().unwrap(),
            "--param",
            "T=3",
            "--param",
            "N=4",
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("HUGRiHJ"));
}

#[test]
fn reports_missing_hugr_parameter() {
    let source = temporary_file("missing.alpha", QUANTUM_CX);
    let output = Command::new(env!("CARGO_BIN_EXE_alphac"))
        .args(["--emit", "hugr", source.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("missing parameter 'N'"));
}

#[test]
fn rejects_duplicate_parameters() {
    let source = temporary_file("duplicate.alpha", QUANTUM_CHAIN);
    let output = Command::new(env!("CARGO_BIN_EXE_alphac"))
        .args([
            "--emit",
            "hugr",
            "--param",
            "N=4",
            "--param",
            "N=5",
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("duplicate parameter 'N'"));
}
