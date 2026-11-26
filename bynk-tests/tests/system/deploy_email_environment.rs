//! `[deploy.email."<address>"]` maps a named environment's inbound-email address (#3883). A
//! Worker accepts mail to each address an environment claims in place of a `from email`
//! service's own, so the generated `email()` entry carries a route for each; and a malformed
//! table is refused. This runs one project through the real `bynk build` and `bynk check`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SUPPORT: &str = "context shop.support\n\nconsumes bynk.cloudflare {\n\tInboundEmail,\n\tEmailVerdict,\n}\n\nservice support from email(\"support@shop.example\") {\n\ton message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {\n\t\tAccept\n\t}\n}\n\nservice sales from email(\"sales@shop.example\") {\n\ton message(mail: InboundEmail) -> Effect[EmailVerdict] by Sender {\n\t\tAccept\n\t}\n}\n";

const BASE: &str = "[build]\ntarget = \"workers\"\n\n";
const MAPPED: &str = "[deploy.email.\"support@shop.example\"]\naddress = { staging = \"support@staging.shop.example\" }\n";

fn run(root: &Path, command: &str) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg(command)
        .arg(root)
        .output()
        .expect("run bynk");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), out)
}

fn write_project(name: &str, manifest: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("bynk-deploy-email-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/shop")).unwrap();
    fs::write(root.join("bynk.toml"), manifest).unwrap();
    fs::write(root.join("src/shop/support.bynk"), SUPPORT).unwrap();
    root
}

/// The text of the Worker entry the build wrote for `shop-support`.
fn entry_of(root: &Path) -> String {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.ends_with("shop-support/index.ts") {
                found.push(path);
            }
        }
    }
    assert_eq!(found.len(), 1, "one shop-support entry: {found:?}");
    fs::read_to_string(&found[0]).unwrap()
}

#[test]
fn a_mapped_address_is_routed_to_the_same_service() {
    let root = write_project("mapped", &format!("{BASE}{MAPPED}"));
    let (ok, out) = run(&root, "build");
    assert!(ok, "{out}");
    let entry = entry_of(&root);
    assert!(
        entry.contains("{ address: \"support@shop.example\", handle: (mail, sender) => surface.queue_support_0(mail, sender) }"),
        "{entry}"
    );
    assert!(
        entry.contains("{ address: \"support@staging.shop.example\", handle: (mail, sender) => surface.queue_support_0(mail, sender) }"),
        "the mapped address reaches the service it stands in for:\n{entry}"
    );
    // The service that is not mapped keeps its one route.
    assert_eq!(entry.matches("surface.queue_sales_0").count(), 1, "{entry}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn without_a_mapping_the_routes_are_the_sources() {
    let root = write_project("plain", BASE);
    let (ok, out) = run(&root, "build");
    assert!(ok, "{out}");
    let entry = entry_of(&root);
    assert_eq!(
        entry.matches("surface.queue_support_0").count(),
        1,
        "{entry}"
    );
    assert!(!entry.contains("staging"), "{entry}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_malformed_mapping_is_refused_with_its_table() {
    let cases = [
        (
            "noenv",
            "[deploy.email.\"support@shop.example\"]\naddress = {}\n",
            "names no environment",
        ),
        (
            "default",
            "[deploy.email.\"support@shop.example\"]\naddress = { default = \"a@b.example\" }\n",
            "`address.default` cannot be mapped",
        ),
        (
            "key",
            "[deploy.email.\"support@shop.example\"]\naddres = { staging = \"a@b.example\" }\n",
            "did you mean `address`?",
        ),
    ];
    for (name, mapping, expected) in cases {
        let root = write_project(name, &format!("{BASE}{mapping}"));
        let (ok, out) = run(&root, "check");
        assert!(!ok, "{name}: {out}");
        assert!(
            out.contains("[deploy.email.\"support@shop.example\"]"),
            "{name}: {out}"
        );
        assert!(out.contains(expected), "{name}: {out}");
        let _ = fs::remove_dir_all(&root);
    }
}
