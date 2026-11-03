//! Ice tiles: the 4-clique motifs of a base topology (FAM-38 bounded
//! traversal).
//!
//! A tile is a 4-clique. Its spine is a perfect matching of the tile that
//! holds every edge the tile shares with another tile. [`topology`] finds
//! the tiles and [`matching`] splits the spine edges into two sides.

pub mod matching;
pub mod topology;

pub use matching::{select, sides, Sides};
pub use topology::{
    candidates, zephyr_coordinates, zephyr_coupler, zephyr_lattice, Candidates, Tile,
};

/// The work bound of candidate search (FAM-38 bounded traversal).
pub const MAX_DEGREE: u64 = 32;

/// How the construction finds tiles (D2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lattice {
    /// Find 4-cliques from the adjacency alone.
    Adjacency,
    /// The topology is a subgraph of the Zephyr lattice `z(m, t)`. Find
    /// tiles from the Zephyr coordinates.
    Zephyr {
        /// The grid size.
        m: u16,
        /// The tile size.
        t: u8,
    },
}

impl Lattice {
    /// `(0, 0)` is [`Lattice::Adjacency`], both nonzero is
    /// [`Lattice::Zephyr`], anything else is a mixed shape.
    ///
    /// # Errors
    ///
    /// [`StructureError::MixedShape`] when exactly one of `m` and `t` is zero.
    pub const fn from_shape(m: u16, t: u8) -> Result<Self, StructureError> {
        match (m, t) {
            (0, 0) => Ok(Self::Adjacency),
            (0, _) | (_, 0) => Err(StructureError::MixedShape),
            _ => Ok(Self::Zephyr { m, t }),
        }
    }
}

/// Why a topology cannot carry ice tiles. Stable: tests and vectors name the
/// variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StructureError {
    /// Exactly one of the Zephyr parameters is zero.
    MixedShape,
    /// The largest degree is above [`MAX_DEGREE`].
    DegreeAboveLimit,
    /// The same unordered pair appears twice.
    DuplicateEdge,
    /// A native id is at or above `4 * t * m * (2m + 1)`.
    NotZephyrNode(u32),
    /// Two native ids, smaller first, that are not a Zephyr coupler.
    NotZephyrCoupler(u32, u32),
    /// The topology has no 4-clique.
    NoCandidates,
    /// The shared edges of a 4-clique are not inside one perfect matching.
    SpineConflict,
    /// A native id is in more than two tile slots.
    MembershipAboveTwo(u32),
    /// The spine graph has an odd cycle.
    NotBipartite,
    /// A select order that is not a permutation of its side.
    BadOrder,
    /// A count does not fit its integer type.
    Overflow,
}

impl StructureError {
    /// The kebab-case name of the variant.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MixedShape => "mixed-shape",
            Self::DegreeAboveLimit => "degree-above-limit",
            Self::DuplicateEdge => "duplicate-edge",
            Self::NotZephyrNode(_) => "not-zephyr-node",
            Self::NotZephyrCoupler(..) => "not-zephyr-coupler",
            Self::NoCandidates => "no-candidates",
            Self::SpineConflict => "spine-conflict",
            Self::MembershipAboveTwo(_) => "membership-above-two",
            Self::NotBipartite => "not-bipartite",
            Self::BadOrder => "bad-order",
            Self::Overflow => "overflow",
        }
    }
}
