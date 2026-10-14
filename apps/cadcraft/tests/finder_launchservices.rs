//! Run with CADCRAFT_MACOS_TEST_APP=/path/to/CADCraft.app cargo test -p cadcraft
//! --test finder_launchservices -- --ignored. Files go through LaunchServices, never argv.
//! The packaged app must include the Finder event adapter from PR #683 (or an equivalent).
#![cfg(target_os = "macos")]

use cadcraft_engine::Session;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct Run {
    root: PathBuf,
    port: u16,
}

impl Run {
    fn request(&self, method: &str, params: Value) -> std::io::Result<Value> {
        let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], self.port).into(), Duration::from_secs(1))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        writeln!(stream, "{}", json!({"id": 1, "method": method, "params": params}))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        serde_json::from_str(&line).map_err(std::io::Error::other)
    }

    fn execute(&self, command: &str, params: Value) -> Value {
        let reply = self.request("engine.execute", json!({"command": command, "params": params})).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"].clone()
    }

    fn until_open(&self, path: &Path) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        let path = path.canonicalize().unwrap();
        loop {
            if let Ok(reply) = self.request("drawing.inspect", json!({}))
                && reply["ok"] == true
                && reply["result"]["path"].as_str().is_some_and(|p| Path::new(p) == path)
            {
                return reply["result"].clone();
            }
            assert!(Instant::now() < deadline, "LaunchServices did not open {}", path.display());
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn drawing(&self, name: &str) -> PathBuf {
        let mut source = Session::new();
        source.execute("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
        let path = self.root.join(name);
        std::fs::write(&path, cadcraft_io::write(source.doc().unwrap(), name).unwrap()).unwrap();
        path
    }

    fn document_count(&self) -> usize {
        for index in 0..64 {
            let reply = self.request("engine.execute", json!({"command": "document.switch", "params": {"index": index}})).unwrap();
            if reply["ok"] == false {
                return index;
            }
        }
        panic!("unexpected number of tabs in the isolated test app");
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // Only this test's private control port and synthetic drawings are touched.
        let _ = self.request("engine.execute", json!({"command": "closeall", "params": {}}));
        let _ = self.request("app.quit", json!({}));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn checked(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "requires a macOS GUI session and CADCRAFT_MACOS_TEST_APP"]
fn cold_launch_and_running_app_receive_finder_events_without_losing_edits() {
    let original = PathBuf::from(std::env::var_os("CADCRAFT_MACOS_TEST_APP").expect("set CADCRAFT_MACOS_TEST_APP to the packaged .app"));
    assert!(original.join("Contents/Info.plist").is_file());
    let root = std::env::temp_dir().join(format!("cadcraft-launchservices-{}", std::process::id()));
    assert!(!root.exists(), "preserve any existing test directory");
    std::fs::create_dir(&root).unwrap();
    let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let run = Run { root, port };
    // A private bundle identifier keeps the user's running CADCraft and associations untouched.
    let app = run.root.join("CADCraft Finder Test.app");
    checked(Command::new("ditto").args(["--norsrc", "--noextattr"]).arg(&original).arg(&app));
    checked(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Set :CFBundleIdentifier ai.storyteller.cadcraft.finder-test.{}", std::process::id())])
            .arg(app.join("Contents/Info.plist")),
    );
    checked(Command::new("codesign").args(["--force", "--deep", "--sign", "-"]).arg(&app));
    let cold = run.drawing("检查井 成果 #1.dwg");
    let warm = run.drawing("管网 检测.dxf");
    checked(
        Command::new("open")
            .args(["-n", "-g", "-a"])
            .arg(&app)
            .arg("--env")
            .arg(format!("CADCRAFT_CONFIG_DIR={}", run.root.join("profile").display()))
            .arg(&cold)
            .args(["--args", "--control", &port.to_string()]),
    );
    assert_eq!(run.until_open(&cold)["entityCount"], 1);
    run.execute("circle", json!({"center": [10, 10], "radius": 2}));
    checked(Command::new("open").args(["-g", "-a"]).arg(&app).arg(&warm));
    assert_eq!(run.until_open(&warm)["entityCount"], 1);
    let count = run.document_count();
    checked(Command::new("open").args(["-g", "-a"]).arg(&app).arg(&cold));
    let reopened = run.until_open(&cold);
    assert_eq!(reopened["entityCount"], 2);
    assert_eq!(reopened["dirty"], true, "Finder must retain unsaved edits");
    assert_eq!(run.document_count(), count, "reopening must not create a duplicate tab");
}
