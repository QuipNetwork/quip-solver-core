//! The square-lattice link graph: signed incidence, direct charges, and
//! charge-request compatibility.
//!
//! A link carries one spin. Spin +1 points from the tail to the head, so the
//! incidence is -1 at the tail and +1 at the head. The charge of a vertex is
//! the sum over its links of incidence times spin. It equals the arrows into
//! the vertex minus the arrows out of it. Nothing in this module reads Ising
//! coefficients or penalties.
//!
//! Two boundaries exist. A periodic lattice of side `L` has `L * L` vertices
//! with index `y * L + x`, and each vertex owns a `+x` link and a `+y` link
//! that wrap around. For `L = 2` the wrap makes parallel links, for example
//! links 0 and 2 both join vertices 0 and 1. An open lattice of `L` cells per
//! side has `(L + 1) * (L + 1)` vertices with index `y * (L + 1) + x`. A
//! vertex owns a `+x` link when `x < L` and a `+y` link when `y < L`. In both
//! cases the links are ordered by owning vertex, `+x` first.
//!
//! The side is `2 <= L <= 64`.
//!
//! The symmetries of a request are the lattice maps that carry the request
//! onto itself, see [`symmetries`].

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

/// The smallest supported side.
const MIN_SIDE: u16 = 2;
/// The largest supported side.
const MAX_SIDE: u16 = 64;

/// How the lattice edge behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    /// Links wrap around, so every vertex has degree 4.
    Periodic,
    /// Links stop at the edge, so boundary vertices have degree 2 or 3.
    Open,
}

/// The direction of a link from its tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// The link goes to the neighbour at `x + 1`.
    X,
    /// The link goes to the neighbour at `y + 1`.
    Y,
}

/// One link. Spin +1 points from the tail to the head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    /// The owning vertex.
    pub tail: u32,
    /// The neighbour of the tail along `axis`.
    pub head: u32,
    /// The direction from the tail to the head.
    pub axis: Axis,
}

/// A lattice cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LatticeError {
    /// The side is outside `2..=64`.
    SizeOutOfRange,
}

impl fmt::Display for LatticeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeOutOfRange => f.write_str("the lattice side is outside 2..=64"),
        }
    }
}

impl core::error::Error for LatticeError {}

/// A square lattice with its links and signed incidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SquareLattice {
    side: u16,
    boundary: Boundary,
    links: Vec<Link>,
    incident: Vec<Vec<(u32, i8)>>,
    /// The link owned by each vertex, by axis (`+x` then `+y`).
    owned: Vec<[Option<u32>; 2]>,
}

impl SquareLattice {
    /// Builds the lattice. The side must be in `2..=64`.
    ///
    /// # Errors
    ///
    /// [`LatticeError::SizeOutOfRange`] when the side is out of range.
    pub fn new(l: u16, boundary: Boundary) -> Result<Self, LatticeError> {
        if !(MIN_SIDE..=MAX_SIDE).contains(&l) {
            return Err(LatticeError::SizeOutOfRange);
        }
        let side = u32::from(l);
        let width = match boundary {
            Boundary::Periodic => side,
            Boundary::Open => side + 1,
        };
        let vertices = width * width;
        let mut links = Vec::new();
        let mut incident = vec![Vec::new(); vertices as usize];
        let mut owned = vec![[None, None]; vertices as usize];
        for y in 0..width {
            for x in 0..width {
                let tail = y * width + x;
                let (east, north) = match boundary {
                    Boundary::Periodic => (Some((x + 1) % width), Some((y + 1) % width)),
                    Boundary::Open => ((x < side).then_some(x + 1), (y < side).then_some(y + 1)),
                };
                let heads = [
                    east.map(|nx| (y * width + nx, Axis::X)),
                    north.map(|ny| (ny * width + x, Axis::Y)),
                ];
                for (head, axis) in heads.into_iter().flatten() {
                    let index =
                        u32::try_from(links.len()).map_err(|_| LatticeError::SizeOutOfRange)?;
                    links.push(Link { tail, head, axis });
                    if let Some(slot) = owned.get_mut(tail as usize) {
                        *slot
                            .get_mut(usize::from(axis == Axis::Y))
                            .unwrap_or(&mut None) = Some(index);
                    }
                    for (vertex, sign) in [(tail, -1), (head, 1)] {
                        if let Some(list) = incident.get_mut(vertex as usize) {
                            list.push((index, sign));
                        }
                    }
                }
            }
        }
        Ok(Self {
            side: l,
            boundary,
            links,
            incident,
            owned,
        })
    }

    /// The side `L` the lattice was built with.
    #[must_use]
    pub const fn side(&self) -> u16 {
        self.side
    }

    /// The boundary the lattice was built with.
    #[must_use]
    pub const fn boundary(&self) -> Boundary {
        self.boundary
    }

    /// The number of vertices.
    #[must_use]
    pub fn vertex_count(&self) -> u32 {
        u32::try_from(self.incident.len()).unwrap_or(u32::MAX)
    }

    /// The number of links.
    #[must_use]
    pub fn link_count(&self) -> u32 {
        u32::try_from(self.links.len()).unwrap_or(u32::MAX)
    }

    /// Link `e`, or `None` past the last link.
    #[must_use]
    pub fn link(&self, e: u32) -> Option<Link> {
        self.links.get(e as usize).copied()
    }

    /// The links at vertex `v` as `(link, incidence)` pairs in ascending link
    /// order. The slice is empty for a vertex that does not exist.
    #[must_use]
    pub fn incident(&self, v: u32) -> &[(u32, i8)] {
        self.incident.get(v as usize).map_or(&[], Vec::as_slice)
    }

    /// The number of links at vertex `v`, or `None` for a vertex that does
    /// not exist.
    #[must_use]
    pub fn degree(&self, v: u32) -> Option<u8> {
        let list = self.incident.get(v as usize)?;
        u8::try_from(list.len()).ok()
    }
}

/// The charge of every vertex for the spins `spins`, one spin per link.
///
/// Returns `None` when `spins` has the wrong length or holds a value other
/// than +1 or -1.
#[must_use]
pub fn charges(lattice: &SquareLattice, spins: &[i8]) -> Option<Vec<i8>> {
    if spins.len() != lattice.links.len() || spins.iter().any(|&s| s != 1 && s != -1) {
        return None;
    }
    lattice
        .incident
        .iter()
        .map(|list| {
            let mut sum = 0i8;
            for &(link, sign) in list {
                let spin = *spins.get(link as usize)?;
                sum = sum.checked_add(sign.checked_mul(spin)?)?;
            }
            Some(sum)
        })
        .collect()
}

/// A request on the charges and the spins of a lattice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// One entry per vertex. `None` leaves the vertex free.
    pub charges: Vec<Option<i8>>,
    /// Links with a fixed spin, as `(link, spin)` in strictly ascending link
    /// order. The spin is +1 or -1.
    pub fixed: Vec<(u32, i8)>,
    /// Empty, or one signed field value per link.
    pub fields: Vec<i32>,
}

/// Why a request is refused. The checks run in the order of the variants and
/// the first failure wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestError {
    /// `charges` does not have one entry per vertex.
    ChargesLength,
    /// `fields` is neither empty nor one value per link.
    FieldsLength,
    /// A fixed link index is not a link.
    FixedOutOfRange,
    /// The fixed links are not strictly ascending.
    FixedNotAscending,
    /// A fixed spin is not +1 or -1.
    FixedSpin,
    /// A label is larger in magnitude than the degree of its vertex.
    DegreeBound,
    /// A label has a different parity from the degree of its vertex.
    Parity,
    /// Every vertex is labeled and the labels do not sum to 0.
    Total,
    /// No orientation meets every label and fixed spin.
    Infeasible,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ChargesLength => "charges do not have one entry per vertex",
            Self::FieldsLength => "fields are neither empty nor one per link",
            Self::FixedOutOfRange => "a fixed link is out of range",
            Self::FixedNotAscending => "fixed links are not strictly ascending",
            Self::FixedSpin => "a fixed spin is not +1 or -1",
            Self::DegreeBound => "a charge label exceeds the vertex degree",
            Self::Parity => "a charge label has the wrong parity",
            Self::Total => "the labels cover every vertex and do not sum to 0",
            Self::Infeasible => "no orientation meets the request",
        })
    }
}

impl core::error::Error for RequestError {}

/// Checks that some orientation of the links meets every labeled charge and
/// every fixed spin.
///
/// The local tests run first, then an exact search for an orientation. The
/// search is a maximum flow. Every free link sends one unit to the endpoint
/// it points into. A labeled vertex must take exactly `(degree + charge) / 2`
/// arrows in, less the arrows the fixed links already give it. A free vertex
/// takes any number up to its free degree. The labeled demands are saturated
/// first and the free vertices are added afterwards. Augmenting paths never
/// reduce the flow on a sink edge, so the first stage stays saturated.
///
/// # Errors
///
/// The first failing check, see [`RequestError`].
pub fn check_request(lattice: &SquareLattice, request: &Request) -> Result<(), RequestError> {
    if request.charges.len() != lattice.incident.len() {
        return Err(RequestError::ChargesLength);
    }
    if !request.fields.is_empty() && request.fields.len() != lattice.links.len() {
        return Err(RequestError::FieldsLength);
    }
    if request
        .fixed
        .iter()
        .any(|&(link, _)| link as usize >= lattice.links.len())
    {
        return Err(RequestError::FixedOutOfRange);
    }
    if request.fixed.windows(2).any(|pair| match pair {
        [(a, _), (b, _)] => a >= b,
        _ => false,
    }) {
        return Err(RequestError::FixedNotAscending);
    }
    if request
        .fixed
        .iter()
        .any(|&(_, spin)| spin != 1 && spin != -1)
    {
        return Err(RequestError::FixedSpin);
    }
    let labels = labeled_degrees(lattice, request)?;
    if labels.iter().any(|&(degree, label)| {
        label.is_some_and(|q| i16::from(q).unsigned_abs() > u16::from(degree))
    }) {
        return Err(RequestError::DegreeBound);
    }
    if labels
        .iter()
        .any(|&(degree, label)| label.is_some_and(|q| (i16::from(q) - i16::from(degree)) % 2 != 0))
    {
        return Err(RequestError::Parity);
    }
    if labels.iter().all(|&(_, label)| label.is_some())
        && labels
            .iter()
            .map(|&(_, label)| i32::from(label.unwrap_or(0)))
            .sum::<i32>()
            != 0
    {
        return Err(RequestError::Total);
    }
    if orientation_exists(lattice, request, &labels) {
        Ok(())
    } else {
        Err(RequestError::Infeasible)
    }
}

/// The degree and label of every vertex.
fn labeled_degrees(
    lattice: &SquareLattice,
    request: &Request,
) -> Result<Vec<(u8, Option<i8>)>, RequestError> {
    request
        .charges
        .iter()
        .enumerate()
        .map(|(v, &label)| {
            let degree = u32::try_from(v)
                .ok()
                .and_then(|v| lattice.degree(v))
                .ok_or(RequestError::ChargesLength)?;
            Ok((degree, label))
        })
        .collect()
}

/// A flow network of unit and small capacities with paired residual edges.
struct Network {
    to: Vec<usize>,
    capacity: Vec<u32>,
    adjacent: Vec<Vec<usize>>,
}

impl Network {
    fn new(nodes: usize) -> Self {
        Self {
            to: Vec::new(),
            capacity: Vec::new(),
            adjacent: vec![Vec::new(); nodes],
        }
    }

    /// Adds an edge and its zero-capacity reverse. Edge `i` pairs with `i ^ 1`.
    fn add_edge(&mut self, from: usize, to: usize, capacity: u32) -> Option<()> {
        let index = self.to.len();
        self.adjacent.get_mut(from)?.push(index);
        self.adjacent.get_mut(to)?.push(index + 1);
        self.to.push(to);
        self.capacity.push(capacity);
        self.to.push(from);
        self.capacity.push(0);
        Some(())
    }

    /// Pushes flow from `source` to `sink` along shortest augmenting paths
    /// until none is left. Returns the added flow.
    fn augment(&mut self, source: usize, sink: usize) -> Option<u32> {
        let mut total = 0u32;
        loop {
            let mut via: Vec<Option<usize>> = vec![None; self.adjacent.len()];
            let mut seen = vec![false; self.adjacent.len()];
            *seen.get_mut(source)? = true;
            let mut queue = alloc::collections::VecDeque::from([source]);
            while let Some(node) = queue.pop_front() {
                if node == sink {
                    break;
                }
                for &edge in self.adjacent.get(node)? {
                    let next = *self.to.get(edge)?;
                    if *self.capacity.get(edge)? > 0 && !*seen.get(next)? {
                        *seen.get_mut(next)? = true;
                        *via.get_mut(next)? = Some(edge);
                        queue.push_back(next);
                    }
                }
            }
            if !*seen.get(sink)? {
                return Some(total);
            }
            // Every capacity on a path is at least 1, and the source edges
            // have capacity 1, so each path carries exactly one unit.
            let mut node = sink;
            while node != source {
                let edge = (*via.get(node)?)?;
                *self.capacity.get_mut(edge)? -= 1;
                *self.capacity.get_mut(edge ^ 1)? += 1;
                node = *self.to.get(edge ^ 1)?;
            }
            total = total.checked_add(1)?;
        }
    }
}

/// Whether some orientation of the non-fixed links gives every labeled vertex
/// its label. The caller has already checked parity and degree bounds.
fn orientation_exists(
    lattice: &SquareLattice,
    request: &Request,
    labels: &[(u8, Option<i8>)],
) -> bool {
    solve(lattice, request, labels).unwrap_or(false)
}

fn solve(lattice: &SquareLattice, request: &Request, labels: &[(u8, Option<i8>)]) -> Option<bool> {
    let vertices = lattice.incident.len();
    let links = lattice.links.len();
    // Arrows into each vertex from the fixed links, and the free degree.
    let mut fixed_in = vec![0i32; vertices];
    let mut is_fixed = vec![false; links];
    for &(link, spin) in &request.fixed {
        let Link { tail, head, .. } = lattice.link(link)?;
        let into = if spin == 1 { head } else { tail };
        *fixed_in.get_mut(into as usize)? += 1;
        *is_fixed.get_mut(link as usize)? = true;
    }
    let mut free_degree = vec![0i32; vertices];
    for (v, list) in lattice.incident.iter().enumerate() {
        let free = list
            .iter()
            .filter(|&&(link, _)| !is_fixed.get(link as usize).copied().unwrap_or(true))
            .count();
        *free_degree.get_mut(v)? = i32::try_from(free).ok()?;
    }

    // Nodes: source 0, sink 1, vertices from 2, then the free links.
    let vertex_node = |v: usize| v + 2;
    let link_node = |e: usize| vertices + 2 + e;
    let mut network = Network::new(vertices + links + 2);
    let mut free_links = 0u32;
    for (e, link) in lattice.links.iter().enumerate() {
        if *is_fixed.get(e)? {
            continue;
        }
        network.add_edge(0, link_node(e), 1)?;
        network.add_edge(link_node(e), vertex_node(link.tail as usize), 1)?;
        network.add_edge(link_node(e), vertex_node(link.head as usize), 1)?;
        free_links += 1;
    }

    // Stage one: the labeled vertices take exactly what they need.
    let mut demand = 0u32;
    for (v, &(degree, label)) in labels.iter().enumerate() {
        let Some(q) = label else { continue };
        let need = i32::midpoint(i32::from(degree), i32::from(q)) - *fixed_in.get(v)?;
        if need < 0 || need > *free_degree.get(v)? {
            return Some(false);
        }
        let need = u32::try_from(need).ok()?;
        network.add_edge(vertex_node(v), 1, need)?;
        demand = demand.checked_add(need)?;
    }
    if network.augment(0, 1)? != demand {
        return Some(false);
    }

    // Stage two: the free vertices take whatever is left.
    for (v, &(_, label)) in labels.iter().enumerate() {
        if label.is_none() {
            network.add_edge(vertex_node(v), 1, u32::try_from(*free_degree.get(v)?).ok()?)?;
        }
    }
    let rest = network.augment(0, 1)?;
    Some(demand.checked_add(rest)? == free_links)
}

/// A 2 by 2 integer matrix, acting as `x' = xx * x + xy * y` and
/// `y' = yx * x + yy * y`.
#[derive(Clone, Copy)]
struct Matrix {
    xx: i32,
    xy: i32,
    yx: i32,
    yy: i32,
}

/// The eight elements of the dihedral group D4. Element 0 is the identity.
const D4: [Matrix; 8] = [
    Matrix {
        xx: 1,
        xy: 0,
        yx: 0,
        yy: 1,
    },
    Matrix {
        xx: 0,
        xy: -1,
        yx: 1,
        yy: 0,
    },
    Matrix {
        xx: -1,
        xy: 0,
        yx: 0,
        yy: -1,
    },
    Matrix {
        xx: 0,
        xy: 1,
        yx: -1,
        yy: 0,
    },
    Matrix {
        xx: -1,
        xy: 0,
        yx: 0,
        yy: 1,
    },
    Matrix {
        xx: 1,
        xy: 0,
        yx: 0,
        yy: -1,
    },
    Matrix {
        xx: 0,
        xy: 1,
        yx: 1,
        yy: 0,
    },
    Matrix {
        xx: 0,
        xy: -1,
        yx: -1,
        yy: 0,
    },
];

/// A lattice map, taken with or without global spin reversal.
///
/// The map sends link (tail `t`, axis `d`) to the link at tail `g(t)` in
/// direction `R d`. When `R d` is negative, the image is the link owned by
/// the neighbour `g(t) + R d`, and the link sign is -1. Links map by tail and
/// axis, never by vertex pair, so parallel links on a small torus stay apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symmetry {
    tx: u16,
    ty: u16,
    element: u8,
    reversed: bool,
}

impl Symmetry {
    /// The translation `(tx, ty)`. It is `(0, 0)` on an open lattice.
    #[must_use]
    pub const fn translation(self) -> (u16, u16) {
        (self.tx, self.ty)
    }

    /// The D4 element index, 0 for the identity.
    #[must_use]
    pub const fn element(self) -> u8 {
        self.element
    }

    /// Whether the candidate also reverses every spin, negating charges.
    #[must_use]
    pub const fn is_reversal(self) -> bool {
        self.reversed
    }

    fn matrix(self) -> Matrix {
        D4.get(usize::from(self.element)).copied().unwrap_or(D4[0])
    }

    fn width(lattice: &SquareLattice) -> i32 {
        match lattice.boundary {
            Boundary::Periodic => i32::from(lattice.side),
            Boundary::Open => i32::from(lattice.side) + 1,
        }
    }

    /// The image of the point `(x, y)`.
    fn map_point(self, lattice: &SquareLattice, px: i32, py: i32) -> (i32, i32) {
        let m = self.matrix();
        let side = i32::from(lattice.side);
        match lattice.boundary {
            Boundary::Periodic => (
                (m.xx * px + m.xy * py + i32::from(self.tx)).rem_euclid(side),
                (m.yx * px + m.yy * py + i32::from(self.ty)).rem_euclid(side),
            ),
            Boundary::Open => {
                // Work in doubled coordinates about the centre of the grid.
                let (u, w) = (2 * px - side, 2 * py - side);
                (
                    (m.xx * u + m.xy * w + side) / 2,
                    (m.yx * u + m.yy * w + side) / 2,
                )
            }
        }
    }

    /// The image of vertex `v`, or `None` for a vertex that does not exist.
    #[must_use]
    pub fn vertex_image(self, lattice: &SquareLattice, v: u32) -> Option<u32> {
        if v as usize >= lattice.incident.len() {
            return None;
        }
        let width = u32::try_from(Self::width(lattice)).ok()?;
        let (x, y) = (
            i32::try_from(v % width).ok()?,
            i32::try_from(v / width).ok()?,
        );
        let (nx, ny) = self.map_point(lattice, x, y);
        u32::try_from(ny * Self::width(lattice) + nx).ok()
    }

    /// The image of link `e` and its sign, +1 or -1, or `None` for a link
    /// that does not exist. A spin `s` on link `e` becomes `sign * s` on the
    /// image link, times -1 more under reversal.
    #[must_use]
    pub fn link_image(self, lattice: &SquareLattice, link_index: u32) -> Option<(u32, i8)> {
        let link = lattice.link(link_index)?;
        let width = u32::try_from(Self::width(lattice)).ok()?;
        let (px, py) = (
            i32::try_from(link.tail % width).ok()?,
            i32::try_from(link.tail / width).ok()?,
        );
        let m = self.matrix();
        let (gx, gy) = self.map_point(lattice, px, py);
        let (dx, dy) = match link.axis {
            Axis::X => (m.xx, m.yx),
            Axis::Y => (m.xy, m.yy),
        };
        // The image axis is Y when the direction has a y component.
        let image_axis = usize::from(dy != 0);
        let (owner_x, owner_y, sign) = if dx + dy > 0 {
            (gx, gy, 1)
        } else {
            (gx + dx, gy + dy, -1)
        };
        let side = i32::from(lattice.side);
        let (owner_x, owner_y) = match lattice.boundary {
            Boundary::Periodic => (owner_x.rem_euclid(side), owner_y.rem_euclid(side)),
            Boundary::Open => (owner_x, owner_y),
        };
        let wide = Self::width(lattice);
        if owner_x < 0 || owner_y < 0 || owner_x >= wide || owner_y >= wide {
            return None;
        }
        let owner = usize::try_from(owner_y * wide + owner_x).ok()?;
        let image = (*lattice.owned.get(owner)?.get(image_axis)?)?;
        Some((image, sign))
    }

    /// The spins of the image witness: spin `s` on link `e` becomes
    /// `sign * r * s` on the image link, with `r = -1` under reversal.
    ///
    /// Returns `None` when `spins` has the wrong length or a value other than
    /// +1 or -1.
    #[must_use]
    pub fn apply(self, lattice: &SquareLattice, spins: &[i8]) -> Option<Vec<i8>> {
        if spins.len() != lattice.links.len() || spins.iter().any(|&s| s != 1 && s != -1) {
            return None;
        }
        let r: i8 = if self.reversed { -1 } else { 1 };
        let mut image = vec![0i8; spins.len()];
        for (e, &spin) in spins.iter().enumerate() {
            let (target, sign) = self.link_image(lattice, u32::try_from(e).ok()?)?;
            *image.get_mut(target as usize)? = sign * r * spin;
        }
        Some(image)
    }

    /// Whether the candidate maps the request onto itself.
    fn preserves(self, lattice: &SquareLattice, request: &Request) -> bool {
        let r: i16 = if self.reversed { -1 } else { 1 };
        for (v, label) in request.charges.iter().enumerate() {
            let Some(image) = u32::try_from(v)
                .ok()
                .and_then(|v| self.vertex_image(lattice, v))
            else {
                return false;
            };
            let expected = label.map(|q| r * i16::from(q));
            let found = request
                .charges
                .get(image as usize)
                .copied()
                .flatten()
                .map(i16::from);
            if expected != found {
                return false;
            }
        }
        for &(link, spin) in &request.fixed {
            let Some((image, sign)) = self.link_image(lattice, link) else {
                return false;
            };
            let want = i16::from(sign) * r * i16::from(spin);
            let has = request
                .fixed
                .binary_search_by_key(&image, |&(l, _)| l)
                .ok()
                .and_then(|i| request.fixed.get(i))
                .map(|&(_, s)| i16::from(s));
            if has != Some(want) {
                return false;
            }
        }
        for (e, &field) in request.fields.iter().enumerate() {
            let Some((image, sign)) = u32::try_from(e)
                .ok()
                .and_then(|e| self.link_image(lattice, e))
            else {
                return false;
            };
            let want = i64::from(sign) * i64::from(r) * i64::from(field);
            if request.fields.get(image as usize).map(|&f| i64::from(f)) != Some(want) {
                return false;
            }
        }
        true
    }
}

/// Every candidate that maps the request onto itself, in a fixed order:
/// translation (`ty * L + tx`), then D4 element, then no reversal before
/// reversal.
///
/// The candidates are the lattice maps with and without global spin
/// reversal, at most `2 * 8 * L^2` on a periodic lattice and 16 on an open
/// one. A candidate is kept when charges, fixed spins, and fields map onto
/// themselves, with charges, fixed spins, and fields negated under reversal.
/// Candidates that act the same way on the links are not merged. On a small
/// torus several do. A request of the wrong shape has no symmetry.
///
/// The cost is at most `2 * 8 * L^2` candidates of `O(links)` work each.
#[must_use]
pub fn symmetries(lattice: &SquareLattice, request: &Request) -> Vec<Symmetry> {
    if request.charges.len() != lattice.incident.len()
        || (!request.fields.is_empty() && request.fields.len() != lattice.links.len())
    {
        return Vec::new();
    }
    let shifts = match lattice.boundary {
        Boundary::Periodic => lattice.side,
        Boundary::Open => 1,
    };
    let mut found = Vec::new();
    for ty in 0..shifts {
        for tx in 0..shifts {
            for element in 0..8u8 {
                for reversed in [false, true] {
                    let candidate = Symmetry {
                        tx,
                        ty,
                        element,
                        reversed,
                    };
                    if candidate.preserves(lattice, request) {
                        found.push(candidate);
                    }
                }
            }
        }
    }
    found
}
