//! Only exercises the paths that don't touch launchd/systemd: printing the
//! unit and uninstalling when nothing is installed.

mod common;

use common::Env;

#[test]
fn install_print_shows_unit_without_installing() {
    let env = Env::new();
    let (code, v) = env.json(&["daemon", "install", "--print"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["installed"], false);
    let content = v["content"].as_str().unwrap();
    assert!(
        content.contains("daemon") && content.contains("run"),
        "{content}"
    );
    // The service must use the same TOME_HOME the installer ran with.
    assert!(content.contains(env.home().to_str().unwrap()), "{content}");
    let path = std::path::PathBuf::from(v["path"].as_str().unwrap());
    assert!(
        path.starts_with(env.dir.path()),
        "unit path should follow $HOME: {path:?}"
    );
    assert!(!path.exists());

    if cfg!(target_os = "macos") {
        assert_eq!(v["platform"], "launchd");
        assert!(content.contains("<key>KeepAlive</key>"));
    } else {
        assert_eq!(v["platform"], "systemd");
        assert!(content.contains("Restart=on-failure"));
    }

    let out = env.run(&["daemon", "install", "--print"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        content.trim_end()
    );
}

#[test]
fn uninstall_when_not_installed_is_a_noop() {
    let env = Env::new();
    let (code, v) = env.json(&["daemon", "uninstall"]);
    assert_eq!(code, 0);
    assert_eq!(v["removed"], false);
}
