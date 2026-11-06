//! `kizuna-verify-pack`: verifies a KIZUNA evidence pack offline (no database, no network).
//!
//! `kizuna-verify-pack receipt <receipt.json> [--file <document>]` verifies a sealed delivery receipt instead.
//!
//! Exit status: 0 verified (receipt: final), 1 verification failed, 2 usage error or unreadable file,
//! 3 receipt pending (consistent, but no signed checkpoint covers it yet).

use std::io::Read;
use std::process::ExitCode;

use kizuna_verify_pack::receipt::{FileDigest, verify_receipt};
use kizuna_verify_pack::{Lang, VerifyOptions, render, render_receipt, verify};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: kizuna-verify-pack <pack.zip> [--trusted-key <public-key-hex|key-id>] [--lang en|ja|both] [--json]\n\
       kizuna-verify-pack receipt <receipt.json> [--file <document>] [--trusted-key ..] [--lang ..] [--json]\n\
使い方: kizuna-verify-pack <pack.zip> [--trusted-key <公開鍵hex|鍵ID>] [--lang en|ja|both] [--json]\n\
        kizuna-verify-pack receipt <受領証.json> [--file <文書>] [--trusted-key ..] [--lang ..] [--json]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut opts = VerifyOptions::default();
    let mut lang = Lang::Both;
    let mut json = false;
    let mut receipt = false;
    let mut file = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "receipt" if !receipt && path.is_none() => receipt = true,
            "--file" if receipt => match args.next() {
                Some(f) => file = Some(f),
                None => return usage(),
            },
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
    if receipt {
        return check_receipt(&path, file.as_deref(), &opts, lang, json);
    }
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

fn digest_file(path: &str) -> std::io::Result<FileDigest> {
    let mut f = std::fs::File::open(path)?;
    let (mut h, mut size, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok(FileDigest {
        sha256: h.finalize().into(),
        size,
    })
}

fn check_receipt(path: &str, file: Option<&str>, opts: &VerifyOptions, lang: Lang, json: bool) -> ExitCode {
    let mut bytes = Vec::new();
    // Read at most one byte past the limit: an oversized receipt is reported, never loaded whole.
    let limit = kizuna_verify_pack::receipt::MAX_RECEIPT_BYTES as u64 + 1;
    if let Err(e) = std::fs::File::open(path).and_then(|f| f.take(limit).read_to_end(&mut bytes)) {
        eprintln!("cannot open {path}: {e} / {path} を開けません: {e}");
        return ExitCode::from(2);
    }
    let digest = match file.map(digest_file).transpose() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read --file: {e} / --file を読めません: {e}");
            return ExitCode::from(2);
        }
    };
    let report = verify_receipt(&bytes, digest, opts);
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(2);
            }
        }
    } else {
        print!("{}", render_receipt(&report, lang));
    }
    match report.status {
        "final" => ExitCode::SUCCESS,
        "pending" => ExitCode::from(3),
        _ => ExitCode::from(1),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
