use std::path::Path;

/// Freezes owned fixture files in an isolated repository without developer hooks or global config.
pub fn snapshot(root: &Path) -> String {
    let git = |args: &[&str]| {
        let mut command = std::process::Command::new("git");
        command.current_dir(root);
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(name);
            }
        }
        let output = command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.join("absent-global-config"))
            .args(["-c", "core.hooksPath=", "-c", "core.fsmonitor=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "--quiet", "--template="]);
    git(&["add", "."]);
    let tree = git(&["write-tree"]);
    let oid = git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit-tree",
        &tree,
        "-m",
        "test(fixture): freeze owned feature inputs",
    ]);
    git(&["update-ref", "HEAD", &oid]);
    oid
}
