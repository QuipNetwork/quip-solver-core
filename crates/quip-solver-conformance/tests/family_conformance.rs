//! SC-7 conformance of generated instances and family sessions. The checker
//! tests run at library level. The `--family` gate tests run the CLI and a
//! binary that never connects.

#[path = "family_conformance/faulty.rs"]
mod faulty;

use faulty::{FAULTY_J_V1, FAULTY_METADATA_V1, FAULTY_OVERSIZE_V1};
use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits, Contract};
use ising_generators::family::{
    admit_topology, generate_instance, BaseTopology, Defect, SaltFault,
};
use ising_generators::instance::Instance;
use ising_generators::random::derive_nonce;
use quip_protocol::lease::salt_with_counter;
use quip_solver_conformance::families::layered::{RESTRICTED_V1, UNIFORM_V1};
use quip_solver_conformance::families::{
    check_case, compare, drive_miner_family, export_instances, first_diff, parse_vector_cases,
    CaseMismatch, Component, Expected, ExpectedSalt, VectorCase,
};
use quip_solver_conformance::solve::CaseOutcome;
use quip_solver_conformance::FAMILY_LAYERED;

/// 2 layers, field -1500.
const P_SPEC: &str = "01010002000000020002000400000024faffff";
/// Lease slot 1 uses counter `5 + 1`. Slot 0 is skipped.
const SLOT_ONE_COUNTER: u64 = 6;

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("bad hex at {i}"))
        })
        .collect()
}

/// The topology `T_SPARSE`.
fn t_sparse() -> Result<BaseTopology, String> {
    let nodes = vec![907, 41, 0, 4_294_967_295, 12, 3];
    let edges = [
        (907, 41),
        (41, 0),
        (0, 4_294_967_295),
        (4_294_967_295, 12),
        (12, 3),
        (3, 907),
        (41, 12),
    ];
    BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))
}

/// Generate the instance of lease slot 1 for `P_SPEC` on `T_SPARSE` under `contract`.
fn generate_slot_one(contract: Contract) -> Result<Result<Instance, SaltFault>, String> {
    let params = hex_decode(P_SPEC)?;
    let contracts = [contract];
    let admitted = admit(
        &contracts,
        contract.family_id.as_bytes(),
        contract.revision,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("admit failed with {c}"))?;
    let topology = t_sparse()?;
    let bounds = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
    .map_err(|c| format!("admit_topology failed with {c}"))?;
    let base_salt = {
        let mut salt = [0x33_u8; 32];
        salt[..8].fill(0);
        salt
    };
    let salt = salt_with_counter(&base_salt, SLOT_ONE_COUNTER);
    let nonce = derive_nonce([0x11; 32], [0x22; 32], salt);
    Ok(generate_instance(&admitted, &topology, &bounds, nonce))
}

fn honest_and_faulty(faulty: Contract) -> Result<(Instance, Instance), String> {
    let honest = generate_slot_one(UNIFORM_V1)?.map_err(|f| format!("honest: {f:?}"))?;
    let broken = generate_slot_one(faulty)?.map_err(|f| format!("faulty: {f:?}"))?;
    Ok((honest, broken))
}

/// The all-+1 energy: offset plus the sum of h plus the sum of J, in milli.
fn all_up_energy(instance: &Instance) -> i64 {
    let h: i64 = instance.h().iter().copied().map(i64::from).sum();
    let j: i64 = instance.j().iter().copied().map(i64::from).sum();
    instance.offset_milli() + h + j
}

fn canonical(instance: &Instance) -> Result<Vec<u8>, String> {
    instance.canonical_bytes().map_err(|e| e.to_string())
}

#[test]
fn faulty_j_keeps_the_all_up_energy_and_changes_the_couplings() -> Result<(), String> {
    let (honest, broken) = honest_and_faulty(FAULTY_J_V1)?;
    let (a, b) = (all_up_energy(&honest), all_up_energy(&broken));
    if a != b {
        return Err(format!("energies differ: {a} and {b}"));
    }
    let sum = |i: &Instance| i.j().iter().copied().map(i64::from).sum::<i64>();
    if sum(&honest) != sum(&broken) {
        return Err("sum of J differs".to_owned());
    }
    if honest.j() == broken.j() {
        return Err("the couplings are equal at slot 1".to_owned());
    }
    if canonical(&honest)? == canonical(&broken)? {
        return Err("canonical bytes are equal".to_owned());
    }
    Ok(())
}

#[test]
fn faulty_metadata_keeps_the_energy_and_changes_the_region() -> Result<(), String> {
    let (honest, broken) = honest_and_faulty(FAULTY_METADATA_V1)?;
    let (a, b) = (all_up_energy(&honest), all_up_energy(&broken));
    if a != b {
        return Err(format!("energies differ: {a} and {b}"));
    }
    if honest.metadata() == broken.metadata() {
        return Err("the metadata regions are equal".to_owned());
    }
    if canonical(&honest)? == canonical(&broken)? {
        return Err("canonical bytes are equal".to_owned());
    }
    Ok(())
}

#[test]
fn faulty_oversize_is_a_size_bound_defect() -> Result<(), String> {
    match generate_slot_one(FAULTY_OVERSIZE_V1)? {
        Err(SaltFault::Defect(Defect::SizeBound)) => Ok(()),
        other => Err(format!("expected a size bound defect, got {other:?}")),
    }
}

fn cases() -> Result<Vec<VectorCase>, String> {
    parse_vector_cases(FAMILY_LAYERED)
}

fn case_named<'a>(cases: &'a [VectorCase], name: &str) -> Result<&'a VectorCase, String> {
    cases
        .iter()
        .find(|case| case.name == name)
        .ok_or_else(|| format!("no case {name}"))
}

fn contract_of(case: &VectorCase) -> Result<Contract, String> {
    match case.family_id.as_str() {
        "vectors/uniform-field" => Ok(UNIFORM_V1),
        "vectors/restricted-field" => Ok(RESTRICTED_V1),
        other => Err(format!("unknown family {other}")),
    }
}

fn nonces_of(case: &VectorCase) -> Vec<[u8; 32]> {
    case.salts.iter().map(|salt| salt.nonce).collect()
}

fn skipped_indexes(case: &VectorCase) -> Vec<u64> {
    case.salts
        .iter()
        .filter(|salt| matches!(salt.expected, Expected::Skip(_)))
        .map(|salt| salt.index)
        .collect()
}

/// The vector slot with index 1 of `uniform-p-spec-sparse`, and its canonical bytes.
fn slot_one(cases: &[VectorCase]) -> Result<(ExpectedSalt, Vec<u8>), String> {
    let case = case_named(cases, "uniform-p-spec-sparse")?;
    let salt = case
        .salts
        .iter()
        .find(|salt| salt.index == 1)
        .ok_or("no slot 1")?;
    let Expected::Instance { canonical, .. } = &salt.expected else {
        return Err("slot 1 is a skip".to_owned());
    };
    Ok((salt.clone(), canonical.clone()))
}

#[test]
fn vector_cases_parse() -> Result<(), String> {
    let cases = cases()?;
    let sizes: Vec<usize> = cases.iter().map(|case| case.salts.len()).collect();
    if sizes != [16, 8, 8] {
        return Err(format!("salt counts {sizes:?}"));
    }
    let names: Vec<&str> = cases.iter().map(|case| case.name.as_str()).collect();
    if names
        != [
            "uniform-p-spec-sparse",
            "uniform-l2-f0-sparse",
            "restricted-p-spec-sparse",
        ]
    {
        return Err(format!("case names {names:?}"));
    }
    let sparse = case_named(&cases, "uniform-p-spec-sparse")?;
    if skipped_indexes(sparse) != [0, 3, 9, 13] {
        return Err(format!("skips {:?}", skipped_indexes(sparse)));
    }
    let indexes: Vec<u64> = sparse.salts.iter().map(|salt| salt.index).collect();
    if indexes != (0..16).collect::<Vec<u64>>() {
        return Err("salts are not sorted by index".to_owned());
    }
    Ok(())
}

#[test]
fn a_malformed_vector_is_an_error() -> Result<(), String> {
    let mut root: serde_json::Value =
        serde_json::from_str(FAMILY_LAYERED).map_err(|e| e.to_string())?;
    let case = root
        .get_mut("layered")
        .and_then(|cases| cases.get_mut(0))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("no case 0")?;
    let _ = case.remove("instances").ok_or("no instances")?;
    let json = root.to_string();
    match parse_vector_cases(&json) {
        Err(message) if message.contains("instances") => {}
        other => return Err(format!("expected an instances error, got {other:?}")),
    }
    let mut root: serde_json::Value =
        serde_json::from_str(FAMILY_LAYERED).map_err(|e| e.to_string())?;
    let hex = root
        .pointer_mut("/layered/0/instances/0/canonical_hex")
        .ok_or("no canonical_hex")?;
    *hex = serde_json::Value::String("abc".to_owned());
    match parse_vector_cases(&root.to_string()) {
        Err(message) if message.contains("canonical_hex") => Ok(()),
        other => Err(format!("expected a canonical_hex error, got {other:?}")),
    }
}

#[test]
fn reference_layered_passes_every_vector_case() -> Result<(), String> {
    for case in cases()? {
        check_case(&contract_of(&case)?, &case)
            .map_err(|m| format!("{}: {:?} {}", case.name, m.salt_index, m.diff))?;
    }
    Ok(())
}

#[test]
fn no_vector_nonce_opens_an_unlisted_label() -> Result<(), String> {
    for case in cases()? {
        let generated = export_instances(
            &contract_of(&case)?,
            &case.params,
            &case.topology,
            &nonces_of(&case),
        )
        .map_err(|c| format!("admission {c}"))?;
        for got in &generated {
            if matches!(got, Err(SaltFault::Defect(Defect::UnlistedLabel))) {
                return Err(format!("{}: unlisted label", case.name));
            }
        }
    }
    Ok(())
}

fn max_degree(instance: &Instance) -> u64 {
    let mut degree = std::collections::BTreeMap::<u32, u64>::new();
    for &(u, v) in instance.edges() {
        *degree.entry(u).or_default() += 1;
        *degree.entry(v).or_default() += 1;
    }
    degree.values().copied().max().unwrap_or(0)
}

#[test]
fn generated_instances_stay_within_size_bounds() -> Result<(), String> {
    for case in cases()? {
        let contract = contract_of(&case)?;
        let contracts = [contract];
        let admitted = admit(
            &contracts,
            case.family_id.as_bytes(),
            case.revision,
            &case.params,
            &params_digest(&case.params),
        )
        .map_err(|c| format!("admit {c}"))?;
        let bounds = admitted
            .size_bounds(case.topology.stats())
            .map_err(|c| format!("bounds {c}"))?;
        let generated =
            export_instances(&contract, &case.params, &case.topology, &nonces_of(&case))
                .map_err(|c| format!("admission {c}"))?;
        for instance in generated.iter().flatten() {
            let nodes = u64::try_from(instance.node_ids().len()).map_err(|e| e.to_string())?;
            let edges = u64::try_from(instance.edges().len()).map_err(|e| e.to_string())?;
            if nodes > bounds.nodes || edges > bounds.edges || max_degree(instance) > bounds.degree
            {
                return Err(format!("{}: beyond {bounds:?}", case.name));
            }
        }
    }
    Ok(())
}

#[test]
fn exported_instances_do_not_depend_on_thread_count() -> Result<(), String> {
    let cases = cases()?;
    let case = case_named(&cases, "uniform-p-spec-sparse")?;
    let nonces = nonces_of(case);
    let export = |nonces: &[[u8; 32]]| {
        export_instances(&UNIFORM_V1, &case.params, &case.topology, nonces)
            .map_err(|c| format!("admission {c}"))
    };
    let single = export(&nonces)?;
    let threaded = std::thread::scope(|scope| {
        let handles: Vec<_> = nonces
            .chunks(2)
            .map(|chunk| scope.spawn(move || export(chunk)))
            .collect();
        let mut all = Vec::new();
        for handle in handles {
            let part = handle
                .join()
                .map_err(|_| "a thread panicked".to_owned())??;
            all.extend(part);
        }
        Ok::<_, String>(all)
    })?;
    if single == threaded {
        Ok(())
    } else {
        Err("1 thread and 8 threads differ".to_owned())
    }
}

#[test]
fn first_diff_names_each_component_in_fam_30_order() -> Result<(), String> {
    let (_, canonical) = slot_one(&cases()?)?;
    if first_diff(&canonical, &canonical).is_some() {
        return Err("equal bytes differ".to_owned());
    }
    let rows = [
        (0, Component::Format, None),
        (1, Component::DescriptorDigest, None),
        (33, Component::TopologyOrderDigest, None),
        (65, Component::Nonce, None),
        (97, Component::NodeCount, None),
        (113, Component::NodeIds, Some(3)),
        (149, Component::EdgeCount, None),
        (169, Component::Edges, Some(2)),
        (313, Component::H, Some(0)),
        (365, Component::J, Some(1)),
        (441, Component::Offset, None),
        (449, Component::MetadataSectionCount, None),
        (453, Component::MetadataSection, Some(0)),
        (457, Component::MetadataSection, Some(0)),
    ];
    for (offset, component, index) in rows {
        let mut flipped = canonical.clone();
        let byte = flipped.get_mut(offset).ok_or("offset out of range")?;
        *byte ^= 0x01;
        let diff =
            first_diff(&canonical, &flipped).ok_or_else(|| format!("no diff at {offset}"))?;
        if diff.component != component || diff.index != index {
            return Err(format!("offset {offset}: {diff}"));
        }
        let mirrored = first_diff(&flipped, &canonical).ok_or("no mirrored diff")?;
        if mirrored.component != component {
            return Err(format!("offset {offset} mirrored: {mirrored}"));
        }
    }
    let mut longer = canonical.clone();
    longer.push(0);
    match first_diff(&canonical, &longer) {
        Some(diff) if diff.component == Component::Length => {}
        other => return Err(format!("appended byte: {other:?}")),
    }
    match first_diff(&canonical, canonical.get(..300).ok_or("short")?) {
        Some(diff) if diff.component == Component::Length => Ok(()),
        other => Err(format!("truncated: {other:?}")),
    }
}

#[test]
fn equal_bytes_with_a_wrong_digest_is_an_instance_digest_difference() -> Result<(), String> {
    let cases = cases()?;
    let case = case_named(&cases, "uniform-p-spec-sparse")?;
    let (mut salt, _) = slot_one(&cases)?;
    let Expected::Instance { digest, .. } = &mut salt.expected else {
        return Err("slot 1 is a skip".to_owned());
    };
    if let Some(byte) = digest.first_mut() {
        *byte ^= 0x01;
    }
    let generated = export_instances(&UNIFORM_V1, &case.params, &case.topology, &[salt.nonce])
        .map_err(|c| format!("admission {c}"))?;
    let got = generated.first().ok_or("no instance")?;
    match compare(&salt, got) {
        Err(diff) if diff.component == Component::InstanceDigest => Ok(()),
        other => Err(format!(
            "expected an instance digest difference, got {other:?}"
        )),
    }
}

#[test]
fn a_skip_where_an_instance_is_expected_is_an_outcome_difference() -> Result<(), String> {
    let cases = cases()?;
    let case = case_named(&cases, "uniform-p-spec-sparse")?;
    let (salt, _) = slot_one(&cases)?;
    let skipped = case
        .salts
        .iter()
        .find(|salt| salt.index == 0)
        .ok_or("no slot 0")?;
    // Slot 0 generates a skip, slot 1 an instance. Cross them.
    let generated = export_instances(
        &UNIFORM_V1,
        &case.params,
        &case.topology,
        &[skipped.nonce, salt.nonce],
    )
    .map_err(|c| format!("admission {c}"))?;
    let (skip_got, instance_got) = (
        generated.first().ok_or("none")?,
        generated.get(1).ok_or("none")?,
    );
    match compare(&salt, skip_got) {
        Err(diff) if diff.component == Component::Outcome && diff.expected == "instance" => {}
        other => return Err(format!("instance expected, skip got: {other:?}")),
    }
    match compare(skipped, instance_got) {
        Err(diff)
            if diff.component == Component::Outcome
                && diff.expected == "skip code 13"
                && diff.got == "instance" => {}
        other => return Err(format!("skip expected, instance got: {other:?}")),
    }
    compare(skipped, skip_got).map_err(|d| format!("matching skip failed: {d}"))
}

#[test]
fn a_contract_without_a_construction_fails_at_admission() -> Result<(), String> {
    let cases = cases()?;
    let case = case_named(&cases, "uniform-p-spec-sparse")?;
    let bare = Contract {
        construction: None,
        ..UNIFORM_V1
    };
    match check_case(&bare, case) {
        Err(CaseMismatch {
            salt_index: None,
            diff,
            ..
        }) if diff.component == Component::Admission => Ok(()),
        other => Err(format!("expected an admission mismatch, got {other:?}")),
    }
}

fn mismatch_of(contract: &Contract) -> Result<CaseMismatch, String> {
    let cases = cases()?;
    let case = case_named(&cases, "uniform-p-spec-sparse")?;
    check_case(contract, case)
        .err()
        .ok_or_else(|| "the faulty generator passed".to_owned())
}

#[test]
fn faulty_j_fails_at_the_j_component_with_equal_energy() -> Result<(), String> {
    let mismatch = mismatch_of(&FAULTY_J_V1)?;
    if mismatch.salt_index != Some(1) || mismatch.diff.component != Component::J {
        return Err(format!("{:?} {}", mismatch.salt_index, mismatch.diff));
    }
    let cases = cases()?;
    let (salt, _) = slot_one(&cases)?;
    let Expected::Instance {
        energy_all_up_milli,
        ..
    } = salt.expected
    else {
        return Err("slot 1 is a skip".to_owned());
    };
    let (_, broken) = honest_and_faulty(FAULTY_J_V1)?;
    if all_up_energy(&broken) == energy_all_up_milli {
        Ok(())
    } else {
        Err("the faulty energy differs from the vector energy".to_owned())
    }
}

#[test]
fn faulty_metadata_fails_at_metadata_section_0() -> Result<(), String> {
    let mismatch = mismatch_of(&FAULTY_METADATA_V1)?;
    if mismatch.diff.component == Component::MetadataSection && mismatch.diff.index == Some(0) {
        Ok(())
    } else {
        Err(format!("{:?} {}", mismatch.salt_index, mismatch.diff))
    }
}

#[test]
fn faulty_oversize_fails_at_the_outcome() -> Result<(), String> {
    let mismatch = mismatch_of(&FAULTY_OVERSIZE_V1)?;
    if mismatch.diff.component == Component::Outcome && mismatch.diff.got.contains("SizeBound") {
        Ok(())
    } else {
        Err(format!("{:?} {}", mismatch.salt_index, mismatch.diff))
    }
}

#[test]
fn first_diff_survives_absurd_counts_and_lengths() -> Result<(), String> {
    // Equal node counts of u32::MAX over a short input: the walk ends at Length.
    let mut huge = vec![0_u8; 97];
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    let mut other = huge.clone();
    other.push(1);
    match first_diff(&huge, &other) {
        Some(diff) if diff.component == Component::Length => {}
        other => return Err(format!("absurd node count: {other:?}")),
    }
    // A metadata section length near u32::MAX on both sides.
    let (_, canonical) = slot_one(&cases()?)?;
    let mut long = canonical.clone();
    long.get_mut(453..457)
        .ok_or("no section length")?
        .copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    match first_diff(&long, &long.clone()) {
        Some(diff) if diff.component == Component::Length => {}
        other => return Err(format!("equal absurd length: {other:?}")),
    }
    match first_diff(&canonical, &long) {
        Some(diff) if diff.component == Component::MetadataSection && diff.index == Some(0) => {
            Ok(())
        }
        other => Err(format!("absurd section length: {other:?}")),
    }
}

#[test]
fn cli_usage_lists_the_family_gate() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_quip-solver-drive"))
        .arg("--family")
        .output()
        .expect("the CLI runs");
    assert_eq!(output.status.code(), Some(64));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("quip-solver-drive --family <solver-bin> <unix://socket>"),
        "{stderr}"
    );
}

#[tokio::test]
async fn a_binary_that_never_connects_fails_the_family_gate() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let socket = format!(
        "unix:///tmp/quip-family-gate-{}-{nanos}.sock",
        std::process::id()
    );
    let start = std::time::Instant::now();
    let report = drive_miner_family("/bin/true", &[], &socket).await;
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
    assert!(!report.is_conformant(), "{}", report.summary());
    assert!(
        matches!(
            report.outcome("hello-protocol-version-2", None),
            Some(CaseOutcome::Failed(_))
        ),
        "{}",
        report.summary()
    );
    assert!(
        matches!(
            report.outcome("clean-exit", None),
            Some(CaseOutcome::Failed(_))
        ),
        "{}",
        report.summary()
    );
    assert!(
        report.cases.iter().all(|case| case.subject.is_none()),
        "{}",
        report.summary()
    );
}

/// Read a repository file at run time, so a published package still builds.
fn read_repo_file(relative: &str) -> Result<String, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// One row of the `## Coverage` table of `docs/family-conformance.md`.
struct CoverageRow {
    id: String,
    rule: String,
    status: String,
    coverage: String,
    evidence: String,
}

/// Parse the rows of the `## Coverage` table: the lines that start with a
/// backticked check id.
fn coverage_rows() -> Result<Vec<CoverageRow>, String> {
    let doc = read_repo_file("docs/family-conformance.md")?;
    let (_, section) = doc
        .split_once("\n## Coverage\n")
        .ok_or("no `## Coverage` section")?;
    let section = section.split("\n## ").next().unwrap_or(section);
    let mut rows = Vec::new();
    for line in section.lines().filter(|line| line.starts_with("| `")) {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let [id, rule, status, coverage, evidence] = cells[..] else {
            return Err(format!("expected 5 cells: {line}"));
        };
        rows.push(CoverageRow {
            id: id.trim_matches('`').to_owned(),
            rule: rule.to_owned(),
            status: status.to_owned(),
            coverage: coverage.to_owned(),
            evidence: evidence.to_owned(),
        });
    }
    Ok(rows)
}

/// Every `SC-7:<id>` after `## Traceability` in `docs/problem-families.md`.
fn spec_sc7_ids() -> Result<Vec<String>, String> {
    let spec = read_repo_file("docs/problem-families.md")?;
    let (_, trace) = spec
        .split_once("\n## Traceability\n")
        .ok_or("no `## Traceability` section")?;
    let mut ids = Vec::new();
    for piece in trace.split("`SC-7:").skip(1) {
        let id = piece.split('`').next().unwrap_or(piece);
        ids.push(id.to_owned());
    }
    Ok(ids)
}

/// `(SC-7 id, FAM rule)` for each SC-7 id in the rows of the Traceability
/// table of `docs/problem-families.md`.
fn spec_sc7_rules() -> Result<Vec<(String, String)>, String> {
    let spec = read_repo_file("docs/problem-families.md")?;
    let (_, trace) = spec
        .split_once("\n## Traceability\n")
        .ok_or("no `## Traceability` section")?;
    let mut rules = Vec::new();
    for line in trace.lines().filter(|line| line.starts_with("| FAM-")) {
        let rule = line
            .trim_matches('|')
            .split('|')
            .next()
            .map(str::trim)
            .ok_or_else(|| format!("no rule cell: {line}"))?;
        for piece in line.split("`SC-7:").skip(1) {
            let id = piece.split('`').next().unwrap_or(piece);
            rules.push((id.to_owned(), rule.to_owned()));
        }
    }
    Ok(rules)
}

/// Each `(id, rule)` of `table` whose rule differs from the rule of that id
/// in `spec`, or that `spec` does not list.
fn rule_mismatches(table: &[(String, String)], spec: &[(String, String)]) -> Vec<String> {
    let mut wrong = Vec::new();
    for (id, rule) in table {
        match spec.iter().find(|(spec_id, _)| spec_id == id) {
            Some((_, spec_rule)) if spec_rule == rule => {}
            Some((_, spec_rule)) => wrong.push(format!("{id}: table {rule}, spec {spec_rule}")),
            None => wrong.push(format!("{id}: not in the spec")),
        }
    }
    wrong
}

#[test]
fn coverage_rules_equal_the_traceability_rules() -> Result<(), String> {
    let table: Vec<(String, String)> = coverage_rows()?
        .into_iter()
        .map(|row| (row.id, row.rule))
        .collect();
    let spec = spec_sc7_rules()?;
    if spec.len() != 44 {
        return Err(format!(
            "expected 44 SC-7 ids in the spec, found {}",
            spec.len()
        ));
    }
    let wrong = rule_mismatches(&table, &spec);
    if !wrong.is_empty() {
        return Err(format!("the Rule column differs from the spec: {wrong:?}"));
    }
    // A wrong FAM number must be caught.
    let mut broken = table;
    if let Some((_, rule)) = broken.first_mut() {
        *rule = "FAM-99".to_owned();
    }
    if rule_mismatches(&broken, &spec).len() != 1 {
        return Err("a wrong FAM number was not caught".to_owned());
    }
    Ok(())
}

#[test]
fn coverage_table_lists_every_sc7_id() -> Result<(), String> {
    let mut table: Vec<String> = coverage_rows()?.into_iter().map(|row| row.id).collect();
    let mut spec = spec_sc7_ids()?;
    if table.len() != 44 || spec.len() != 44 {
        return Err(format!(
            "expected 44 ids: table has {}, spec has {}",
            table.len(),
            spec.len()
        ));
    }
    table.sort();
    spec.sort();
    if let Some([id, _]) = table
        .windows(2)
        .find(|pair| matches!(pair, [a, b] if a == b))
    {
        return Err(format!("the table repeats {id}"));
    }
    if table != spec {
        let missing: Vec<&String> = spec.iter().filter(|id| !table.contains(id)).collect();
        let extra: Vec<&String> = table.iter().filter(|id| !spec.contains(id)).collect();
        return Err(format!(
            "missing from the table {missing:?}, extra {extra:?}"
        ));
    }
    Ok(())
}

#[test]
fn coverage_rows_follow_the_status_rules() -> Result<(), String> {
    let rows = coverage_rows()?;
    for row in &rows {
        let (coverage, evidence_ok) = match row.status.as_str() {
            "covered" => (
                "host-only",
                !row.evidence.starts_with("needs ") && !row.evidence.starts_with("review: "),
            ),
            "review only" => ("none", row.evidence.starts_with("review: ")),
            "not covered" => ("none", row.evidence.starts_with("needs ")),
            other => return Err(format!("{}: unknown status `{other}`", row.id)),
        };
        if row.coverage != coverage {
            return Err(format!(
                "{}: status `{}` needs coverage `{coverage}`, found `{}`",
                row.id, row.status, row.coverage
            ));
        }
        if !evidence_ok {
            return Err(format!(
                "{}: evidence does not match status `{}`",
                row.id, row.status
            ));
        }
    }
    let count = |status: &str| rows.iter().filter(|row| row.status == status).count();
    let (covered, review, missing) = (count("covered"), count("review only"), count("not covered"));
    if (covered, review, missing) != (25, 2, 17) {
        return Err(format!(
            "expected 25 covered, 2 review only, and 17 not covered, found {covered}, {review}, and {missing}"
        ));
    }
    Ok(())
}

#[test]
fn covered_rows_cite_tests_that_exist() -> Result<(), String> {
    for row in coverage_rows()?
        .iter()
        .filter(|row| row.status == "covered")
    {
        let items: Vec<&str> = row
            .evidence
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|item| item.contains("::"))
            .collect();
        if items.is_empty() {
            return Err(format!("{}: no `path::test` item in the evidence", row.id));
        }
        for item in items {
            let (path, name) = item
                .split_once("::")
                .ok_or_else(|| format!("{}: bad item {item}", row.id))?;
            let source = read_repo_file(path).map_err(|e| format!("{}: {e}", row.id))?;
            if !source.contains(&format!("fn {name}(")) {
                return Err(format!("{}: {path} has no `fn {name}(`", row.id));
            }
        }
    }
    Ok(())
}
