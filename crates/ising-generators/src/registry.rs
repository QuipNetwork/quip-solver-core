//! The family revisions this build implements.

use crate::defects::DEFECTS_V1;
use crate::descriptor::Contract;
use crate::flow::FLOW_V1;
use crate::ice::{CHARGED_ICE_V1, ICE_FIELDS_V1, ICE_GLASS_V1, TWISTED_DIMER_V1};
use crate::polariton::POLARITON_V1;
use crate::routing::ROUTING_V1;

/// Every contract with a construction in this crate, ordered by
/// `(family_id, revision)`. Pass it to [`crate::descriptor::admit`].
pub const CONTRACTS: &[Contract] = &[
    CHARGED_ICE_V1,
    DEFECTS_V1,
    FLOW_V1,
    ICE_FIELDS_V1,
    ICE_GLASS_V1,
    POLARITON_V1,
    TWISTED_DIMER_V1,
    ROUTING_V1,
];
