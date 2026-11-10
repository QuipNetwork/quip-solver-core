//! Family support a binary advertises: its stable `solver_id` and the
//! revisions it lists in `generated_families` and `accepted_families`
//! (docs/problem-families.md, FAM-56 to FAM-63). Answered without a device.

use std::fmt;
use std::sync::Arc;

use ising_generators::descriptor::{admit, params_digest, Contract, Params};
use ising_generators::family::Construction;
use ising_generators::instance::{Instance, InstanceLimits, InstanceParts};
use ising_generators::Code;
use quip_proto::v1::{ising_problem, FamilySupport, IsingProblem};
use quip_protocol::family::{entry_claims, solver_id_class, support_code, SolverIdClass};

/// One `FamilySupport` entry as a binary declares it.
#[derive(Clone, Copy, Debug)]
pub struct FamilyClaim {
    /// The contract of each claimed revision. At least one, all with the same
    /// `family_id`, no revision twice. Each revision is an exact match (FAM-07).
    pub revisions: &'static [Contract],
    /// Canonical parameter bytes this claim is limited to. Empty claims every
    /// valid parameter set of each listed revision (FAM-61). Each set must
    /// pass the schema and bounds of every listed revision. The advertised
    /// `params_digests` are their FAM-18 digests, in this order.
    pub only_params: &'static [&'static [u8]],
}

/// Which `Capabilities` list a claim belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FamilyList {
    /// `generated_families`, field 18.
    Generated,
    /// `accepted_families`, field 19.
    Accepted,
}

impl FamilyList {
    const fn field(self) -> &'static str {
        match self {
            Self::Generated => "generated_families",
            Self::Accepted => "accepted_families",
        }
    }
}

/// The `solver_id` and family lists a binary advertises.
///
/// List a revision in `generated` only after it passes SC-7 conformance on
/// this binary (FAM-57) as `docs/family-conformance.md` defines it, and never
/// an archived revision (FAM-59). The SDK cannot check either rule. A revision missing from `accepted` is served at
/// the generic tier: the plain model is solved and the coordinator decides
/// acceptance (FAM-55, FAM-69).
///
/// The session admits family leases for the `generated` revisions. It checks
/// the `FamilyBinding` of an explicit job for an `accepted` revision in the
/// FAM-65 order before the sampler sees it, and solves every other explicit
/// family job as a plain model. Every claimed contract needs a
/// [`Construction`].
#[derive(Clone, Copy, Debug)]
pub struct Declaration {
    /// Stable id across releases (FAM-60), or `""` for no pin target.
    pub solver_id: &'static str,
    /// Revisions this binary expands from family leases.
    pub generated: &'static [FamilyClaim],
    /// Revisions whose `FamilyBinding` this binary validates.
    pub accepted: &'static [FamilyClaim],
}

impl Declaration {
    /// Generic tier for every revision: no `solver_id`, nothing listed. An
    /// empty list never means every family.
    pub const GENERIC: Self = Self {
        solver_id: "",
        generated: &[],
        accepted: &[],
    };

    /// Check this declaration and build its wire form.
    ///
    /// # Errors
    /// Returns the first [`DeclarationError`], in this order: the `solver_id`,
    /// then each generated claim, then each accepted claim. Within a claim:
    /// no revision, mixed families, a repeated revision, then each parameter
    /// set against each revision, then each contract's construction: missing
    /// or with an invalid label. Last, any entry that FAM-61 says claims
    /// nothing.
    pub fn validate(&self) -> Result<FamilyTable, DeclarationError> {
        if solver_id_class(self.solver_id.as_bytes()) == SolverIdClass::Invalid {
            return Err(DeclarationError::InvalidSolverId(self.solver_id));
        }
        let generated = build_list(FamilyList::Generated, self.generated)?;
        let accepted = build_list(FamilyList::Accepted, self.accepted)?;
        check_entries(FamilyList::Generated, self.generated, &generated)?;
        check_entries(FamilyList::Accepted, self.accepted, &accepted)?;
        Ok(FamilyTable {
            solver_id: self.solver_id,
            generated,
            accepted,
            generated_contracts: contracts_of(self.generated),
            accepted_contracts: contracts_of(self.accepted),
        })
    }
}

fn build_list(
    list: FamilyList,
    claims: &[FamilyClaim],
) -> Result<Vec<FamilySupport>, DeclarationError> {
    claims
        .iter()
        .enumerate()
        .map(|(index, claim)| build_entry(list, index, claim))
        .collect()
}

fn build_entry(
    list: FamilyList,
    index: usize,
    claim: &FamilyClaim,
) -> Result<FamilySupport, DeclarationError> {
    let Some(first) = claim.revisions.first() else {
        return Err(DeclarationError::NoRevision { list, index });
    };
    if claim
        .revisions
        .iter()
        .any(|c| c.family_id != first.family_id)
    {
        return Err(DeclarationError::MixedFamilies { list, index });
    }
    for (at, contract) in claim.revisions.iter().enumerate() {
        if claim
            .revisions
            .iter()
            .take(at)
            .any(|c| c.revision == contract.revision)
        {
            return Err(DeclarationError::RepeatedRevision {
                list,
                index,
                revision: contract.revision,
            });
        }
    }
    for (param, bytes) in claim.only_params.iter().enumerate() {
        for contract in claim.revisions {
            Params::parse(contract.schema, bytes)
                .and_then(|parsed| parsed.check_bounds())
                .map_err(|code| DeclarationError::ParamsRejected {
                    list,
                    index,
                    param,
                    revision: contract.revision,
                    code,
                })?;
        }
    }
    for contract in claim.revisions {
        let Some(construction) = contract.construction else {
            return Err(DeclarationError::NoConstruction {
                list,
                index,
                revision: contract.revision,
            });
        };
        if !labels_valid(construction) {
            return Err(DeclarationError::InvalidLabel {
                list,
                index,
                revision: contract.revision,
            });
        }
    }
    Ok(FamilySupport {
        family_id: first.family_id.to_owned(),
        revisions: claim.revisions.iter().map(|c| c.revision).collect(),
        params_digests: claim
            .only_params
            .iter()
            .map(|bytes| params_digest(bytes).to_vec())
            .collect(),
    })
}

/// Each label is 1 to 32 ASCII bytes and none is listed twice.
fn labels_valid(construction: &Construction) -> bool {
    construction.labels.iter().enumerate().all(|(at, label)| {
        (1..=32).contains(&label.len())
            && label.is_ascii()
            && !construction.labels.iter().take(at).any(|l| l == label)
    })
}

fn contracts_of(claims: &'static [FamilyClaim]) -> Vec<&'static Contract> {
    claims.iter().flat_map(|c| c.revisions).collect()
}

fn find_contract(
    contracts: &[&'static Contract],
    family_id: &str,
    revision: u32,
) -> Option<&'static Contract> {
    contracts
        .iter()
        .find(|c| c.family_id == family_id && c.revision == revision)
        .copied()
}

fn check_entries(
    list: FamilyList,
    claims: &[FamilyClaim],
    built: &[FamilySupport],
) -> Result<(), DeclarationError> {
    let verdicts = entry_claims(built);
    for (index, (claim, ok)) in claims.iter().zip(verdicts).enumerate() {
        if !ok {
            let family_id = claim.revisions.first().map_or("", |c| c.family_id);
            return Err(DeclarationError::InvalidEntry {
                list,
                index,
                family_id,
            });
        }
    }
    Ok(())
}

/// A [`Declaration`] that passed [`Declaration::validate`], in wire form.
/// Built only by `validate`. Two tables are equal when their wire forms are
/// equal.
#[derive(Clone, Debug)]
pub struct FamilyTable {
    solver_id: &'static str,
    generated: Vec<FamilySupport>,
    accepted: Vec<FamilySupport>,
    generated_contracts: Vec<&'static Contract>,
    accepted_contracts: Vec<&'static Contract>,
}

impl PartialEq for FamilyTable {
    fn eq(&self, other: &Self) -> bool {
        self.solver_id == other.solver_id
            && self.generated == other.generated
            && self.accepted == other.accepted
    }
}

impl FamilyTable {
    /// `Capabilities.solver_id`.
    #[must_use]
    pub const fn solver_id(&self) -> &'static str {
        self.solver_id
    }

    /// `Capabilities.generated_families`.
    #[must_use]
    pub fn generated(&self) -> &[FamilySupport] {
        &self.generated
    }

    /// `Capabilities.accepted_families`.
    #[must_use]
    pub fn accepted(&self) -> &[FamilySupport] {
        &self.accepted
    }

    /// The contract of a generated `(family_id, revision)`, exact match
    /// (FAM-07).
    #[must_use]
    pub fn generated_contract(&self, family_id: &str, revision: u32) -> Option<&'static Contract> {
        find_contract(&self.generated_contracts, family_id, revision)
    }

    /// The contract of an accepted `(family_id, revision)`.
    #[must_use]
    pub fn accepted_contract(&self, family_id: &str, revision: u32) -> Option<&'static Contract> {
        find_contract(&self.accepted_contracts, family_id, revision)
    }
}

/// Validated family structure for one job. Only the session builds one, and
/// only after the instance passed every check (FAM-36, FAM-65 to FAM-68).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FamilyContext {
    contract: &'static Contract,
    params: Arc<[u8]>,
    instance: Arc<Instance>,
    source: FamilySource,
}

/// Where a [`FamilyContext`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FamilySource {
    /// The session regenerated the instance of one lease salt.
    Lease {
        /// The salt index within the lease.
        salt_index: u64,
    },
    /// The coordinator sent the instance with a `FamilyBinding`.
    ExplicitJob,
}

impl FamilyContext {
    /// Wrap an instance the session has validated against `contract`.
    pub(crate) fn new(
        contract: &'static Contract,
        params: Arc<[u8]>,
        instance: Arc<Instance>,
        source: FamilySource,
    ) -> Self {
        Self {
            contract,
            params,
            instance,
            source,
        }
    }

    /// The contract the instance was validated against.
    #[must_use]
    pub const fn contract(&self) -> &'static Contract {
        self.contract
    }

    /// The family id of the contract.
    #[must_use]
    pub const fn family_id(&self) -> &'static str {
        self.contract.family_id
    }

    /// The revision of the contract.
    #[must_use]
    pub const fn revision(&self) -> u32 {
        self.contract.revision
    }

    /// The canonical parameter bytes of the descriptor.
    #[must_use]
    pub fn params(&self) -> &[u8] {
        &self.params
    }

    /// The validated instance.
    #[must_use]
    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The offset of the instance, in milli. The session adds it to the energy
    /// of a lease salt (FAM-52) and never to an explicit job (FAM-64).
    #[must_use]
    pub fn offset_milli(&self) -> i64 {
        self.instance.offset_milli()
    }

    /// The payload of metadata section `kind`, from the validated region.
    #[must_use]
    pub fn section(&self, kind: u16) -> Option<&[u8]> {
        metadata_section(self.instance.metadata(), kind)
    }

    /// Where this context came from.
    #[must_use]
    pub const fn source(&self) -> FamilySource {
        self.source
    }
}

/// FAM-65 on an explicit job whose plain model passed every plain-job check
/// (G9). `edges` are the resolved dense edges of the plain model.
///
/// `Ok(None)` is the generic tier: no binding, a binding without a
/// descriptor (G12), or a `(family_id, revision)` that `accepted_families`
/// does not list (FAM-69). `Ok(Some)` is a binding that passed every check.
///
/// # Errors
///
/// The first failing code, in the FAM-65 order: 7, 8, 9, then 10 from the
/// accepted entry; 13 for a FAM-37 bound (G10); 15 for the `topology_hash`
/// graph form (FAM-64), a FAM-46 field length, an edge fault or a digest
/// mismatch; 16 for the metadata layout or the construction's metadata check.
pub(crate) fn check_binding(
    table: &FamilyTable,
    ising: &IsingProblem,
    edges: &[(usize, usize)],
) -> Result<Option<FamilyContext>, Code> {
    let Some(binding) = ising.family.as_ref() else {
        return Ok(None);
    };
    let Some(family) = binding.family.as_ref() else {
        return Ok(None);
    };
    // Codes 5 and 6 from the accepted list make this solver generic for the
    // job. A held code 10 loses to codes 7 to 9.
    let restricted = match support_code(
        table.accepted(),
        &family.family_id,
        family.revision,
        &family.params_digest,
    ) {
        Some(Code::ParamsRestricted) => true,
        Some(_) => return Ok(None),
        None => false,
    };
    // Declaration::validate makes both lookups below unreachable. The release
    // build falls back to the generic tier.
    let Some(contract) = table.accepted_contract(&family.family_id, family.revision) else {
        debug_assert!(
            false,
            "support_code admitted a pair with no accepted contract"
        );
        return Ok(None);
    };
    let Some(construction) = contract.construction else {
        debug_assert!(false, "an accepted contract has no construction");
        return Ok(None);
    };

    // Step 1: the descriptor.
    let admitted = admit(
        std::slice::from_ref(contract),
        family.family_id.as_bytes(),
        family.revision,
        &family.params,
        &family.params_digest,
    )?;
    if restricted {
        return Err(Code::ParamsRestricted);
    }

    // Step 2: the FAM-37 bounds on the rebuilt instance. A field of the wrong
    // length is read as zeros here; step 3 rejects it.
    let mismatch = Code::InstanceDigestMismatch;
    // The plain model already decoded, so a failure here is a value that does
    // not decode to the canonical milli coefficient (FAM-64).
    let (h, j) = crate::encoding::decode_milli(ising).map_err(|_| mismatch)?;
    let digest = |bytes: &[u8]| <[u8; 32]>::try_from(bytes).ok();
    let position = |p: usize| u32::try_from(p).map_err(|_| mismatch);
    let parts = InstanceParts {
        descriptor_digest: admitted.descriptor().digest(),
        topology_order_digest: digest(&binding.topology_order_digest).unwrap_or([0; 32]),
        nonce: digest(&binding.nonce),
        node_ids: binding.node_ids.clone(),
        edges: edges
            .iter()
            .map(|&(u, v)| Ok((position(u)?, position(v)?)))
            .collect::<Result<_, Code>>()?,
        h,
        j,
        offset_milli: binding.offset_milli,
        metadata: binding.metadata.clone(),
    };
    parts.check_bounds(&InstanceLimits::FAM_37)?;

    // Step 3: the graph form, the FAM-46 lengths, then the edges and the
    // digest inside `Instance::new`, which also runs the step 4 layout check.
    let edges_form = matches!(ising.graph, Some(ising_problem::Graph::Edges(_)));
    let lengths_ok = (binding.nonce.is_empty() || binding.nonce.len() == 32)
        && binding.topology_order_digest.len() == 32
        && binding.node_ids.len() == parts.h.len();
    let sent_digest = digest(&binding.instance_digest)
        .filter(|_| edges_form && lengths_ok)
        .ok_or(mismatch)?;
    let instance = Instance::new(
        parts,
        &InstanceLimits::FAM_37,
        contract.metadata_kinds,
        Some(&sent_digest),
    )?;

    // Step 4: the metadata against the instance (FAM-36, FAM-67).
    if !(construction.check_metadata)(admitted.params(), &instance) {
        return Err(Code::MetadataInvalid);
    }
    Ok(Some(FamilyContext::new(
        contract,
        Arc::from(family.params.as_slice()),
        Arc::new(instance),
        FamilySource::ExplicitJob,
    )))
}

/// The payload of section `kind` in a metadata region that passed
/// `validate_metadata`. Returns `None` for a kind that is absent, and for a
/// region that is not well formed.
fn metadata_section(region: &[u8], kind: u16) -> Option<&[u8]> {
    let count = u16::from_le_bytes(region.get(..2)?.try_into().ok()?);
    let mut pos: usize = 2;
    for _ in 0..count {
        let found = u16::from_le_bytes(region.get(pos..pos.checked_add(2)?)?.try_into().ok()?);
        let len_at = pos.checked_add(2)?;
        let length = u32::from_le_bytes(
            region
                .get(len_at..len_at.checked_add(4)?)?
                .try_into()
                .ok()?,
        );
        let start = pos.checked_add(6)?;
        let end = start.checked_add(usize::try_from(length).ok()?)?;
        if found == kind {
            return region.get(start..end);
        }
        pos = end;
    }
    None
}

/// Why a [`Declaration`] cannot be advertised (FAM-62: a solver must not
/// advertise an invalid list).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeclarationError {
    /// `solver_id` is not empty and breaks FAM-60.
    InvalidSolverId(&'static str),
    /// A claim lists no revision.
    NoRevision {
        /// The list holding the claim.
        list: FamilyList,
        /// The position of the claim in its list.
        index: usize,
    },
    /// A claim mixes contracts of different families.
    MixedFamilies {
        /// The list holding the claim.
        list: FamilyList,
        /// The position of the claim in its list.
        index: usize,
    },
    /// A claim lists one revision twice.
    RepeatedRevision {
        /// The list holding the claim.
        list: FamilyList,
        /// The position of the claim in its list.
        index: usize,
        /// The revision listed twice.
        revision: u32,
    },
    /// A restricted parameter set fails a listed revision: code 7 (layout)
    /// or code 9 (bounds).
    ParamsRejected {
        /// The list holding the claim.
        list: FamilyList,
        /// The position of the claim in its list.
        index: usize,
        /// The position of the parameter set in `only_params`.
        param: usize,
        /// The revision that rejects the set.
        revision: u32,
        /// Why the revision rejects it.
        code: Code,
    },
    /// A claimed contract has no construction.
    NoConstruction {
        /// The list holding the claim.
        list: FamilyList,
        /// The position of the claim in its list.
        index: usize,
        /// The revision without a construction.
        revision: u32,
    },
    /// A construction label is not 1 to 32 ASCII bytes, or is listed twice.
    InvalidLabel {
        /// The list holding the claim.
        list: FamilyList,
        /// The position of the claim in its list.
        index: usize,
        /// The revision whose construction has the label.
        revision: u32,
    },
    /// An entry claims nothing under FAM-61: bad `family_id` or the `example`
    /// namespace, revision 0, or a second entry for the same `family_id`.
    InvalidEntry {
        /// The list holding the entry.
        list: FamilyList,
        /// The position of the entry in its list.
        index: usize,
        /// The `family_id` of the entry.
        family_id: &'static str,
    },
}

impl fmt::Display for DeclarationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSolverId(id) => write!(f, "solver_id {id:?} breaks FAM-60"),
            Self::NoRevision { list, index } => {
                write!(f, "{}[{index}] lists no revision", list.field())
            }
            Self::MixedFamilies { list, index } => {
                write!(f, "{}[{index}] mixes families", list.field())
            }
            Self::RepeatedRevision {
                list,
                index,
                revision,
            } => write!(
                f,
                "{}[{index}] lists revision {revision} twice",
                list.field()
            ),
            Self::ParamsRejected {
                list,
                index,
                param,
                revision,
                code,
            } => write!(
                f,
                "{}[{index}] parameter set {param} fails revision {revision} with code {}",
                list.field(),
                *code as u8
            ),
            Self::NoConstruction {
                list,
                index,
                revision,
            } => write!(
                f,
                "{}[{index}] revision {revision} has no construction",
                list.field()
            ),
            Self::InvalidLabel {
                list,
                index,
                revision,
            } => write!(
                f,
                "{}[{index}] revision {revision} has an invalid construction label",
                list.field()
            ),
            Self::InvalidEntry {
                list,
                index,
                family_id,
            } => write!(
                f,
                "{}[{index}] ({family_id:?}) claims nothing under FAM-61",
                list.field()
            ),
        }
    }
}

impl std::error::Error for DeclarationError {}

#[cfg(test)]
mod context_tests {
    use super::*;
    use ising_generators::instance::{InstanceLimits, InstanceParts};

    static CONTRACT: Contract = Contract {
        family_id: "vectors/uniform-field",
        revision: 1,
        schema: quip_solver_conformance::families::layered::UNIFORM_FIELD,
        metadata_kinds: &[1, 3],
        limits: InstanceLimits::FAM_37,
        size_bounds: quip_solver_conformance::families::layered::example_size_bounds,
        construction: Some(&quip_solver_conformance::families::layered::LAYERED),
    };

    /// Two sections: kind 1 with `[7, 8, 9]` and kind 3 with `[5]`.
    const REGION: [u8; 18] = [2, 0, 1, 0, 3, 0, 0, 0, 7, 8, 9, 3, 0, 1, 0, 0, 0, 5];

    fn context() -> FamilyContext {
        let instance = Instance::new(
            InstanceParts {
                descriptor_digest: [0; 32],
                topology_order_digest: [0; 32],
                nonce: None,
                node_ids: vec![0, 1],
                edges: vec![(0, 1)],
                h: vec![0, 0],
                j: vec![0],
                offset_milli: 750,
                metadata: REGION.to_vec(),
            },
            &InstanceLimits::FAM_37,
            &[1, 3],
            None,
        )
        .expect("valid instance");
        FamilyContext::new(
            &CONTRACT,
            Arc::from(vec![1u8, 2, 3]),
            Arc::new(instance),
            FamilySource::Lease { salt_index: 4 },
        )
    }

    #[test]
    fn accessors_report_what_the_constructor_got() {
        let c = context();
        assert_eq!(c.offset_milli(), 750);
        assert_eq!(c.params(), &[1, 2, 3]);
        assert_eq!(c.instance().node_ids(), &[0, 1]);
        assert_eq!(c.source(), FamilySource::Lease { salt_index: 4 });
        assert_eq!((c.family_id(), c.revision()), ("vectors/uniform-field", 1));
    }

    #[test]
    fn section_returns_present_kinds_and_none_for_absent() {
        let c = context();
        let rows = [
            (1, Some(&[7u8, 8, 9][..])),
            (3, Some(&[5u8][..])),
            (2, None),
        ];
        for (kind, expected) in rows {
            assert_eq!(c.section(kind), expected, "kind {kind}");
        }
    }

    #[test]
    fn a_truncated_region_gives_none_without_panicking() {
        for len in 0..REGION.len() {
            let cut = REGION.get(..len).unwrap_or_default();
            // A cut inside the second section must not hide the intact first one.
            let first = metadata_section(cut, 1);
            if len >= 11 {
                assert_eq!(first, Some(&[7u8, 8, 9][..]), "len {len}");
            } else {
                assert_eq!(first, None, "len {len}");
            }
            assert_eq!(metadata_section(cut, 3), None, "len {len}");
        }
    }
}
