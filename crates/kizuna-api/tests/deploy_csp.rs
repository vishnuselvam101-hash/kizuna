//! Review M5-M7 L7: the web app must be served with a strict CSP and no framing. The static files are served by the
//! reverse proxy, so this checks every committed proxy configuration (deploy smoke: `curl -sI https://host/`).

fn configs() -> Vec<(String, String)> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    ["docs/deploy/DEPLOYMENT.md", "deploy/Caddyfile.pilot"]
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(format!("{root}/{p}"))
                .ok()
                .map(|s| (p.to_string(), s))
        })
        .collect()
}

#[test]
fn served_web_app_has_csp_and_frame_ancestors_none() {
    let cs = configs();
    assert!(!cs.is_empty());
    for (p, s) in cs {
        let csp = s
            .lines()
            .find(|l| l.contains("Content-Security-Policy"))
            .unwrap_or_else(|| panic!("{p}: no Content-Security-Policy"));
        for d in [
            "default-src 'self'",
            "script-src 'self'",
            "frame-ancestors 'none'",
            "object-src 'none'",
            "base-uri 'none'",
        ] {
            assert!(csp.contains(d), "{p}: CSP lacks {d}");
        }
        assert!(
            !csp.contains("script-src 'self' 'unsafe-inline'") && !csp.contains("unsafe-eval"),
            "{p}: weak CSP"
        );
        assert!(s.contains("X-Frame-Options DENY"), "{p}: X-Frame-Options");
    }
}

#[test]
fn built_index_has_no_inline_script() {
    let html = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/index.html")).unwrap();
    for l in html.lines().filter(|l| l.contains("<script")) {
        assert!(l.contains(" src="), "inline script would be blocked by the CSP: {l}");
    }
}
