//! `kizuna-verify-pack`: verifies a KIZUNA evidence pack offline (no database, no network).
//!
//! Exit status: 0 verified, 1 verification failed, 2 usage error or unreadable file.

use std::process::ExitCode;

use kizuna_verify_pack::{Lang, VerifyOptions, render, verify};

const USAGE: &str = "usage: kizuna-verify-pack <pack.zip> [--trusted-key <public-key-hex|key-id>] [--lang en|ja|both] [--json]\n\
使い方: kizuna-verify-pack <pack.zip> [--trusted-key <公開鍵hex|鍵ID>] [--lang en|ja|both] [--json]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut opts = VerifyOptions::default();
    let mut lang = Lang::Both;
    let mut json = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--trusted-key" => match args.next() {
                Some(k) => opts.trusted_key = Some(k),
                None => return usage(),
            },
            "--lang" => {
                lang = match args.next().as_deref() {
                    Some("en") => Lang::En,
                    Some("ja") => Lang::Ja,
                    Some("both") => Lang::Both,
                    _ => return usage(),
                }
            }
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if path.is_none() && !a.starts_with('-') => path = Some(a),
            _ => return usage(),
        }
    }
    let Some(path) = path else { return usage() };
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot open {path}: {e} / {path} を開けません: {e}");
            return ExitCode::from(2);
        }
    };
    let report = verify(std::io::BufReader::new(file), &opts);
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(2);
            }
        }
    } else {
        print!("{}", render(&report, lang));
    }
    if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
