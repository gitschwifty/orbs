use assert_cmd::Command;
use predicates::prelude::*;

fn cmd(state: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("orbs").unwrap();
    cmd.arg("--state-dir").arg(state);
    cmd
}

fn create(state: &std::path::Path, title: &str, args: &[&str]) -> String {
    let output = cmd(state)
        .arg("create")
        .arg(title)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    stdout
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Created ")
        .unwrap()
        .to_string()
}

#[test]
fn init_creates_local_store_files() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".orbs");

    cmd(&state)
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized"));

    assert!(state.join("orbs.jsonl").is_file());
    assert!(state.join("deps.jsonl").is_file());
    assert!(state.join("events.jsonl").is_file());
}

#[test]
fn create_list_show_update_delete_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".orbs");

    let id = create(
        &state,
        "Write README",
        &[
            "--description",
            "Document the package",
            "--type",
            "docs",
            "--priority",
            "2",
            "--label",
            "docs",
        ],
    );

    cmd(&state)
        .arg("list")
        .arg("--label")
        .arg("docs")
        .assert()
        .success()
        .stdout(predicate::str::contains("Write README"))
        .stdout(predicate::str::contains("[docs]"));

    cmd(&state)
        .arg("show")
        .arg(&id)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Description: Document the package",
        ));

    cmd(&state)
        .arg("update")
        .arg(&id)
        .arg("--priority")
        .arg("1")
        .arg("--add-label")
        .arg("release")
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated"));

    cmd(&state)
        .arg("list")
        .arg("--label")
        .arg("release")
        .assert()
        .success()
        .stdout(predicate::str::contains("p1 Write README"));

    cmd(&state)
        .arg("delete")
        .arg(&id)
        .arg("--reason")
        .arg("done elsewhere")
        .assert()
        .success()
        .stdout(predicate::str::contains("Deleted"));

    cmd(&state)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("No orbs"));

    cmd(&state)
        .arg("list")
        .arg("--all")
        .assert()
        .success()
        .stdout(predicate::str::contains("Write README"));
}

#[test]
fn deps_ready_waiting_pipeline_and_tree_work() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".orbs");

    let blocker = create(&state, "Set up schema", &[]);
    let blocked = create(
        &state,
        "Build UI",
        &["--parent", &blocker, "--label", "frontend"],
    );

    cmd(&state)
        .arg("dep")
        .arg("add")
        .arg(&blocker)
        .arg(&blocked)
        .assert()
        .success()
        .stdout(predicate::str::contains("Added"));

    cmd(&state)
        .arg("waiting")
        .assert()
        .success()
        .stdout(predicate::str::contains("Build UI"));

    cmd(&state)
        .arg("ready")
        .assert()
        .success()
        .stdout(predicate::str::contains("Set up schema"))
        .stdout(predicate::str::contains("Build UI").not());

    cmd(&state)
        .arg("pipeline")
        .assert()
        .success()
        .stdout(predicate::str::contains("Set up schema"))
        .stdout(predicate::str::contains("Build UI"));

    cmd(&state)
        .arg("tree")
        .arg(&blocker)
        .assert()
        .success()
        .stdout(predicate::str::contains("Set up schema"))
        .stdout(predicate::str::contains("Build UI"));

    cmd(&state)
        .arg("deps")
        .arg(&blocked)
        .assert()
        .success()
        .stdout(predicate::str::contains("blocks"));
}
