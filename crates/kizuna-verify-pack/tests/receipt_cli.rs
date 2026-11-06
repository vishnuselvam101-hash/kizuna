#![allow(clippy::unwrap_used)]
//! `kizuna-verify-pack receipt`: exit status 0 final, 3 pending, 1 failed, 2 usage/unreadable; `--file` and `--json`.

#[path = "common/receipt_fixture.rs"]
mod fixture;

use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> (i32, String) {
    // target/<profile>/deps/<this test> -> target/<profile>/kizuna-verify-pack (built by `cargo test` with the tests).
    let exe = option_env!("CARGO_BIN_EXE_kizuna-verify-pack")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let me = std::env::current_exe().unwrap();
            me.parent().unwrap().parent().unwrap().join("kizuna-verify-pack")
        });
    let o = Command::new(exe).args(args).output().unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
    )
}

#[test]
fn exit_codes_follow_receipt_status() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("receipt-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut seen = 0;
    for (name, bytes, file) in fixture::vectors() {
        let rp = dir.join(format!("{name}.json"));
        std::fs::write(&rp, &bytes).unwrap();
        let mut args = vec![
            "receipt".to_string(),
            rp.to_string_lossy().into_owned(),
            "--lang".into(),
            "en".into(),
        ];
        if let Some(f) = &file {
            let fp = dir.join(format!("{name}.pdf"));
            std::fs::write(&fp, f).unwrap();
            args.extend(["--file".into(), fp.to_string_lossy().into_owned()]);
        }
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, out) = run(&a);
        let want = match name {
            "delivery_pending" => 3,
            n if n.contains("wrong") || n == "bit_flipped" || n == "checkpoint_before_event" || n == "garbage" => 1,
            _ => 0,
        };
        assert_eq!(code, want, "{name}: {out}");
        let head = match code {
            0 => "RESULT: VERIFIED",
            3 => "RESULT: PENDING",
            _ => "RESULT: VERIFICATION FAILED",
        };
        assert!(out.starts_with(head), "{name}: {out}");
        seen += 1;
    }
    assert_eq!(seen, 10);
    let final_path = dir.join("delivery_final.json");
    let (code, out) = run(&["receipt", final_path.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], "final");
    assert_eq!(v["summary"]["request_id"], fixture::REQUEST.to_string());
    assert_eq!(run(&["receipt", dir.join("missing.json").to_str().unwrap()]).0, 2);
    assert_eq!(
        run(&["receipt", final_path.to_str().unwrap(), "--file", "/nonexistent/x"]).0,
        2
    );
    assert_eq!(run(&["receipt"]).0, 2);
    std::fs::remove_dir_all(&dir).unwrap();
}
