//! Family support a binary advertises: its stable `solver_id` and the
//! revisions it lists in `generated_families` and `accepted_families`
//! (docs/problem-families.md, FAM-56 to FAM-63). Answered without a device.

use std::fmt;

use ising_generators::descriptor::{params_digest, Contract, Params};
use ising_generators::family::Construction;
use ising_generators::Code;
use quip_proto::v1::FamilySupport;
use quip_protocol::family::{entry_claims, solver_id_class, SolverIdClass};

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
/// this binary (FAM-57), and never an archived revision (FAM-59). The SDK
/// cannot check either rule. A revision missing from `accepted` is served at
/// the generic tier: the plain model is solved and the coordinator decides
/// acceptance (FAM-55, FAM-69).
///
/// The session admits family leases for the `generated` revisions. It does
/// not yet check `FamilyBinding`s for the `accepted` revisions (FAM-65 to
/// FAM-68): every explicit family job is solved as a plain model. Every
/// claimed contract needs a [`Construction`].
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
