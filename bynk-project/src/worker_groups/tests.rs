use super::*;

fn read(text: &str) -> Result<WorkerGroups, ProjectPathsError> {
    let doc: toml::Table = text.parse().unwrap();
    deploy_workers_from(doc["deploy"].as_table().unwrap())
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_group_lists_contexts_and_may_name_and_route_its_worker() {
    let groups = read(
        "[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pricing\"]\n\n\
         [deploy.workers.payments]\ncontexts = [\"shop.payment\"]\nname = \"acme-payments\"\n\
         routes = [\"pay.example.com/*\", \"*.pay.example.com/v1/*\"]\n\
         custom_domains = [\"pay.example.com\"]\n",
    )
    .unwrap();
    assert_eq!(groups["core"].contexts, ["shop.orders", "shop.pricing"]);
    assert_eq!(groups["core"].script_name("core"), "core");
    assert_eq!(groups["payments"].script_name("payments"), "acme-payments");
    assert_eq!(groups["payments"].routes.len(), 2);
    assert_eq!(groups["payments"].custom_domains, ["pay.example.com"]);
}

#[test]
fn no_table_is_no_groups() {
    assert!(read("[deploy]\n").unwrap().is_empty());
}

#[test]
fn a_malformed_group_is_refused_with_its_table() {
    for (text, needle) in [
        ("[deploy.workers.a]\n", "needs `contexts`"),
        (
            "[deploy.workers.a]\ncontexts = []\n",
            "bynk.deploy.empty_group",
        ),
        (
            "[deploy.workers.a]\ncontexts = \"shop.orders\"\n",
            "must be a list of strings",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"shop.*\"]\n",
            "globs are not supported",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"*\"]\n",
            "not a qualified context name",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\", \"x.y\"]\n",
            "lists context `x.y` twice",
        ),
        (
            "[deploy.workers.a]\ncontextz = [\"x.y\"]\n",
            "did you mean `contexts`",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\nname = \"Bad_Name\"\n",
            "`name` must be a Worker name",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\nroutes = [\"https://a.com/*\"]\n",
            "not a route pattern",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\nroutes = [\"a.com\"]\n",
            "not a route pattern",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\ncustom_domains = [\"*.a.com\"]\n",
            "not a hostname",
        ),
        ("[deploy.workers]\na = 1\n", "must be a table"),
        (
            "[deploy.workers.Shop_Core]\ncontexts = [\"x.y\"]\n",
            "not a valid Worker name",
        ),
    ] {
        let err = read(text).unwrap_err().to_string();
        assert!(err.contains("[deploy.workers."), "{err}");
        assert!(err.contains(needle), "{needle}: {err}");
    }
}

#[test]
fn the_rules_across_groups() {
    for (text, needle) in [
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\n[deploy.workers.b]\ncontexts = [\"x.z\", \"x.y\"]\n",
            "bynk.deploy.context_in_two_groups",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\n[deploy.workers.b]\ncontexts = [\"x.z\"]\nname = \"a\"\n",
            "bynk.deploy.group_name_conflict",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\nroutes = [\"a.com/*\"]\n[deploy.workers.b]\ncontexts = [\"x.z\"]\nroutes = [\"a.com/*\"]\n",
            "bynk.deploy.duplicate_hostname",
        ),
        (
            "[deploy.workers.a]\ncontexts = [\"x.y\"]\ncustom_domains = [\"a.com\"]\n[deploy.workers.b]\ncontexts = [\"x.z\"]\ncustom_domains = [\"a.com\"]\n",
            "bynk.deploy.duplicate_hostname",
        ),
    ] {
        let err = read(text).unwrap_err().to_string();
        assert!(err.contains(needle), "{needle}: {err}");
    }
    // The same host with different paths is two routes, not one claimed twice.
    assert!(
        read("[deploy.workers.a]\ncontexts = [\"x.y\"]\nroutes = [\"a.com/a/*\", \"a.com/b/*\"]\n")
            .is_ok()
    );
}

#[test]
fn groups_are_checked_against_the_projects_units() {
    let contexts = set(&["shop.orders", "shop.pricing", "shop.payment"]);
    let others = set(&["shop.money"]);
    let units = ProjectUnits {
        contexts: &contexts,
        other_units: &others,
    };
    let codes = |text: &str| -> Vec<&'static str> {
        validate_against(&read(text).unwrap(), &units)
            .iter()
            .map(|i| i.code)
            .collect()
    };
    assert!(
        codes("[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pricing\"]\n").is_empty()
    );
    assert_eq!(
        codes("[deploy.workers.core]\ncontexts = [\"shop.ordres\"]\n"),
        ["bynk.deploy.unknown_context"]
    );
    assert_eq!(
        codes("[deploy.workers.core]\ncontexts = [\"shop.money\"]\n"),
        ["bynk.deploy.not_a_context"]
    );
    // `shop-payment` is the implicit Worker of the ungrouped `shop.payment`.
    assert_eq!(
        codes("[deploy.workers.shop-payment]\ncontexts = [\"shop.orders\"]\n"),
        ["bynk.deploy.group_name_conflict"]
    );
    // A singleton group may carry the dashed name of its own context.
    assert!(
        codes(
            "[deploy.workers.shop-payment]\ncontexts = [\"shop.payment\"]\nname = \"acme-pay\"\n"
        )
        .is_empty()
    );
    let issues = validate_against(
        &read("[deploy.workers.core]\ncontexts = [\"shop.ordres\"]\n").unwrap(),
        &units,
    );
    assert!(
        issues[0].message.contains("did you mean `shop.orders`"),
        "{issues:?}"
    );
}

#[test]
fn the_manifest_validator_reads_the_table() {
    // `[deploy.workers]` is a known `[deploy]` key, so the whole-manifest check
    // accepts it and refuses a bad group.
    let root = std::env::temp_dir().join(format!("bynk-wg-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("bynk.toml"),
        "[deploy.workers.core]\ncontexts = []\n",
    )
    .unwrap();
    let err = try_read_worker_groups(&root).unwrap_err().to_string();
    assert!(err.contains("bynk.deploy.empty_group"), "{err}");
    std::fs::remove_dir_all(&root).ok();
}
