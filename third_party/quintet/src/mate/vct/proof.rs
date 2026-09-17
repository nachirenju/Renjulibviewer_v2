use super::state::VCTState;
use crate::board::Point;
use crate::mate::state::State;
use lru::LruCache;
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;
use std::fmt;

const SELECTION_CACHE_CAPACITY: usize = 1000;
const MAX_PRIORITY_PENALTY: u32 = 6;

pub trait ProofTree {
    fn attacker_table(&mut self) -> &mut Table;
    fn defender_table(&mut self) -> &mut Table;
}

pub struct Table {
    table: FxHashMap<u64, Node>,
    selection_cache: LruCache<u64, SelectionSummary>,
    selection_dependents: FxHashMap<u64, Vec<SelectionDependency>>,
    next_selection_generation: u64,
    selection_stats: TableSelectionStats,
}

impl Table {
    pub fn new() -> Self {
        Self {
            table: FxHashMap::default(),
            selection_cache: LruCache::new(SELECTION_CACHE_CAPACITY),
            selection_dependents: FxHashMap::default(),
            next_selection_generation: 0,
            selection_stats: TableSelectionStats::default(),
        }
    }

    pub fn insert(&mut self, state: &VCTState, node: Node) {
        let key = state.zobrist_hash();
        self.insert_key(key, node);
    }

    fn insert_key(&mut self, key: u64, node: Node) {
        if self.table.insert(key, node) == Some(node) {
            return;
        }

        let mut updates = 0u64;
        if let Some(dependents) = self.selection_dependents.get(&key) {
            // selection_dependents is read-only during propagation, so walk
            // the existing slice directly instead of allocating and copying a
            // temporary Vec for every TT update.
            for dependency in dependents.iter().copied() {
                let Some(summary) = self.selection_cache.peek_mut(&dependency.parent_key) else {
                    continue;
                };
                if summary.generation != dependency.generation
                    || summary
                        .candidate_hashes
                        .get(dependency.candidate_index)
                        .copied()
                        != Some(key)
                {
                    continue;
                }
                if summary.replace_node(dependency.candidate_index, node) {
                    updates = updates.saturating_add(1);
                }
            }
        }
        self.selection_stats.incremental_updates = self
            .selection_stats
            .incremental_updates
            .saturating_add(updates);
    }

    pub fn lookup_next(&self, state: &mut VCTState, next_move: Option<Point>) -> Option<Node> {
        let key = state.next_zobrist_hash(next_move);
        self.lookup_key(key)
    }

    pub fn lookup_key(&self, key: u64) -> Option<Node> {
        self.table.get(&key).copied()
    }

    pub fn select_cached<F, I>(
        &mut self,
        parent_key: u64,
        candidate_count: usize,
        kind: SelectionKind,
        base_limit: u8,
        max_priority: u8,
        candidates: F,
    ) -> CachedSelection
    where
        F: FnOnce() -> I,
        I: IntoIterator<Item = (u64, u8)>,
    {
        debug_assert!(candidate_count > 0);
        if let Some(summary) = self.selection_cache.get(&parent_key) {
            if summary.kind == kind
                && summary.base_limit == base_limit
                && summary.max_priority == max_priority
                && summary.candidate_hashes.len() == candidate_count
            {
                self.selection_stats.cache_hits = self.selection_stats.cache_hits.saturating_add(1);
                return summary.snapshot();
            }
        }

        if let Some(old) = self.selection_cache.pop(&parent_key) {
            self.unregister_selection(parent_key, &old);
        }

        self.next_selection_generation = self.next_selection_generation.wrapping_add(1);
        let generation = self.next_selection_generation;
        let mut hashes = Vec::with_capacity(candidate_count);
        let mut nodes = Vec::with_capacity(candidate_count);
        for (hash, priority) in candidates() {
            debug_assert!(priority <= max_priority);
            hashes.push(hash);
            nodes.push(self.table.get(&hash).copied().unwrap_or_else(|| {
                kind.initial_node(candidate_count, base_limit, max_priority, priority)
            }));
        }
        self.selection_stats.full_scans = self.selection_stats.full_scans.saturating_add(1);
        self.selection_stats.child_lookups = self
            .selection_stats
            .child_lookups
            .saturating_add(candidate_count as u64);

        let summary =
            SelectionSummary::new(kind, base_limit, max_priority, hashes, nodes, generation);
        self.register_selection(parent_key, &summary);
        let snapshot = summary.snapshot();
        if let Some((evicted_key, evicted)) = self.selection_cache.push(parent_key, summary) {
            self.unregister_selection(evicted_key, &evicted);
        }
        snapshot
    }

    pub fn selection_stats(&self) -> TableSelectionStats {
        self.selection_stats
    }

    fn register_selection(&mut self, parent_key: u64, summary: &SelectionSummary) {
        for (candidate_index, &child_hash) in summary.candidate_hashes.iter().enumerate() {
            self.selection_dependents
                .entry(child_hash)
                .or_default()
                .push(SelectionDependency {
                    parent_key,
                    candidate_index,
                    generation: summary.generation,
                });
        }
    }

    fn unregister_selection(&mut self, parent_key: u64, summary: &SelectionSummary) {
        for &child_hash in &summary.candidate_hashes {
            let remove_key =
                if let Some(dependents) = self.selection_dependents.get_mut(&child_hash) {
                    dependents.retain(|dependency| {
                        dependency.parent_key != parent_key
                            || dependency.generation != summary.generation
                    });
                    dependents.is_empty()
                } else {
                    false
                };
            if remove_key {
                self.selection_dependents.remove(&child_hash);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableSelectionStats {
    pub full_scans: u64,
    pub cache_hits: u64,
    pub incremental_updates: u64,
    pub child_lookups: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Attack,
    Defence,
}

impl SelectionKind {
    fn initial_node(
        self,
        candidate_count: usize,
        base_limit: u8,
        max_priority: u8,
        priority: u8,
    ) -> Node {
        // Unknown leaves remain finite. The bounded difference only lets a
        // higher-potential candidate consume a little more of the standard
        // second-best threshold before DF-PN moves to the next candidate.
        let penalty = u32::from(max_priority.saturating_sub(priority)).min(MAX_PRIORITY_PENALTY);
        let estimate = (candidate_count as u32).saturating_add(penalty);
        match self {
            Self::Attack => Node::unit_dn(estimate, base_limit),
            Self::Defence => Node::unit_pn(estimate, base_limit),
        }
    }

    fn primary(self, node: Node) -> u32 {
        match self {
            Self::Attack => node.pn,
            Self::Defence => node.dn,
        }
    }

    fn summed(self, node: Node) -> u32 {
        match self {
            Self::Attack => node.dn,
            Self::Defence => node.pn,
        }
    }

    fn default_next(self, base_limit: u8) -> Node {
        match self {
            Self::Attack => Node::zero_dn(base_limit),
            Self::Defence => Node::zero_pn(base_limit),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedSelection {
    pub best_index: usize,
    pub current: Node,
    pub next1: Node,
    pub next2: Node,
}

#[derive(Clone, Copy)]
struct SelectionDependency {
    parent_key: u64,
    candidate_index: usize,
    generation: u64,
}

struct SelectionSummary {
    kind: SelectionKind,
    base_limit: u8,
    max_priority: u8,
    candidate_hashes: Vec<u64>,
    nodes: Vec<Node>,
    primary: BTreeSet<(u32, usize)>,
    limits: BTreeSet<(u8, usize)>,
    finite_sum: u64,
    infinite_sum_count: usize,
    generation: u64,
}

impl SelectionSummary {
    fn new(
        kind: SelectionKind,
        base_limit: u8,
        max_priority: u8,
        candidate_hashes: Vec<u64>,
        nodes: Vec<Node>,
        generation: u64,
    ) -> Self {
        let mut result = Self {
            kind,
            base_limit,
            max_priority,
            candidate_hashes,
            nodes,
            primary: BTreeSet::new(),
            limits: BTreeSet::new(),
            finite_sum: 0,
            infinite_sum_count: 0,
            generation,
        };
        for index in 0..result.nodes.len() {
            result.add_node(index, result.nodes[index]);
        }
        result
    }

    fn replace_node(&mut self, index: usize, node: Node) -> bool {
        let old = self.nodes[index];
        if old == node {
            return false;
        }
        self.remove_node(index, old);
        self.nodes[index] = node;
        self.add_node(index, node);
        true
    }

    fn add_node(&mut self, index: usize, node: Node) {
        let primary = self.kind.primary(node);
        if primary != INF {
            self.primary.insert((primary, index));
        }
        self.limits.insert((node.limit, index));
        let summed = self.kind.summed(node);
        if summed == INF {
            self.infinite_sum_count += 1;
        } else {
            self.finite_sum = self.finite_sum.saturating_add(summed as u64);
        }
    }

    fn remove_node(&mut self, index: usize, node: Node) {
        let primary = self.kind.primary(node);
        if primary != INF {
            self.primary.remove(&(primary, index));
        }
        self.limits.remove(&(node.limit, index));
        let summed = self.kind.summed(node);
        if summed == INF {
            self.infinite_sum_count -= 1;
        } else {
            self.finite_sum -= summed as u64;
        }
    }

    fn snapshot(&self) -> CachedSelection {
        let default_next = self.kind.default_next(self.base_limit);
        let mut ordered = self.primary.iter();
        let first = ordered.next().copied();
        let second = ordered.next().copied();
        let best_index = first.map(|(_, index)| index).unwrap_or(0);
        let next1 = first
            .map(|(_, index)| self.nodes[index])
            .unwrap_or(default_next);
        let next2 = second
            .map(|(_, index)| self.nodes[index])
            .unwrap_or(default_next);
        let primary = first.map(|(value, _)| value).unwrap_or(INF);
        if primary == 0 {
            return self.terminal_snapshot(best_index);
        }

        let limit = self
            .limits
            .iter()
            .next()
            .map(|&(value, _)| value)
            .unwrap_or(self.base_limit)
            .min(self.base_limit);
        let summed = if self.infinite_sum_count > 0 || self.finite_sum >= INF as u64 {
            INF
        } else {
            self.finite_sum as u32
        };

        let current = match self.kind {
            SelectionKind::Attack => Node::new(primary, summed, limit),
            SelectionKind::Defence => Node::new(summed, primary, limit),
        };
        CachedSelection {
            best_index,
            current,
            next1,
            next2,
        }
    }

    fn terminal_snapshot(&self, terminal_index: usize) -> CachedSelection {
        let mut best_index = 0;
        let mut current = self.kind.default_next(self.base_limit);
        let mut next1 = self.kind.default_next(self.base_limit);
        let mut next2 = self.kind.default_next(self.base_limit);
        for (index, &child) in self.nodes[..=terminal_index].iter().enumerate() {
            match self.kind {
                SelectionKind::Attack => {
                    current = Node::new(
                        current.pn.min(child.pn),
                        current.dn.saturating_add(child.dn),
                        current.limit.min(child.limit),
                    );
                    if child.pn < next1.pn {
                        best_index = index;
                        next2 = next1;
                        next1 = child;
                    } else if child.pn < next2.pn {
                        next2 = child;
                    }
                    if current.pn == 0 {
                        current.dn = INF;
                        break;
                    }
                }
                SelectionKind::Defence => {
                    current = Node::new(
                        current.pn.saturating_add(child.pn),
                        current.dn.min(child.dn),
                        current.limit.min(child.limit),
                    );
                    if child.dn < next1.dn {
                        best_index = index;
                        next2 = next1;
                        next1 = child;
                    } else if child.dn < next2.dn {
                        next2 = child;
                    }
                    if current.dn == 0 {
                        current.pn = INF;
                        break;
                    }
                }
            }
        }
        CachedSelection {
            best_index,
            current,
            next1,
            next2,
        }
    }
}

#[cfg(test)]
mod selection_cache_tests {
    use super::*;

    fn reference_selection(
        table: &Table,
        candidates: &[(u64, u8)],
        kind: SelectionKind,
        base_limit: u8,
    ) -> CachedSelection {
        let max_priority = candidates.first().map(|candidate| candidate.1).unwrap_or(0);
        let mut best_index = 0;
        let mut current = kind.default_next(base_limit);
        let mut next1 = kind.default_next(base_limit);
        let mut next2 = kind.default_next(base_limit);
        for (index, &(hash, priority)) in candidates.iter().enumerate() {
            let child = table.table.get(&hash).copied().unwrap_or_else(|| {
                kind.initial_node(candidates.len(), base_limit, max_priority, priority)
            });
            match kind {
                SelectionKind::Attack => {
                    current = Node::new(
                        current.pn.min(child.pn),
                        current.dn.saturating_add(child.dn),
                        current.limit.min(child.limit),
                    );
                    if child.pn < next1.pn {
                        best_index = index;
                        next2 = next1;
                        next1 = child;
                    } else if child.pn < next2.pn {
                        next2 = child;
                    }
                    if current.pn == 0 {
                        current.dn = INF;
                        break;
                    }
                }
                SelectionKind::Defence => {
                    current = Node::new(
                        current.pn.saturating_add(child.pn),
                        current.dn.min(child.dn),
                        current.limit.min(child.limit),
                    );
                    if child.dn < next1.dn {
                        best_index = index;
                        next2 = next1;
                        next1 = child;
                    } else if child.dn < next2.dn {
                        next2 = child;
                    }
                    if current.dn == 0 {
                        current.pn = INF;
                        break;
                    }
                }
            }
        }
        CachedSelection {
            best_index,
            current,
            next1,
            next2,
        }
    }

    fn assert_cached_matches_reference(
        table: &mut Table,
        parent_key: u64,
        candidates: &[(u64, u8)],
        kind: SelectionKind,
        base_limit: u8,
    ) {
        let expected = reference_selection(table, candidates, kind, base_limit);
        let max_priority = candidates.first().map(|candidate| candidate.1).unwrap_or(0);
        let actual = table.select_cached(
            parent_key,
            candidates.len(),
            kind,
            base_limit,
            max_priority,
            || candidates.iter().copied(),
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn attack_selection_cache_matches_full_rescan_after_each_child_update() {
        let mut table = Table::new();
        let candidates = [(10, 9), (11, 7), (12, 5), (13, 3)];
        let parent_key = 100;
        assert_cached_matches_reference(
            &mut table,
            parent_key,
            &candidates,
            SelectionKind::Attack,
            5,
        );

        for (key, node) in [
            (11, Node::new(2, 4, 4)),
            (12, Node::new(2, 7, 3)),
            (13, Node::new(0, INF, 1)),
            (11, Node::new(0, INF, 2)),
            (10, Node::new(INF, 0, 5)),
        ] {
            table.insert_key(key, node);
            assert_cached_matches_reference(
                &mut table,
                parent_key,
                &candidates,
                SelectionKind::Attack,
                5,
            );
        }

        let stats = table.selection_stats();
        assert_eq!(stats.full_scans, 1);
        assert_eq!(stats.cache_hits, 5);
        assert_eq!(stats.incremental_updates, 5);
        assert_eq!(stats.child_lookups, candidates.len() as u64);
    }

    #[test]
    fn defence_selection_cache_matches_full_rescan_after_each_child_update() {
        let mut table = Table::new();
        let candidates = [(20, 10), (21, 8), (22, 6), (23, 4)];
        let parent_key = 200;
        assert_cached_matches_reference(
            &mut table,
            parent_key,
            &candidates,
            SelectionKind::Defence,
            4,
        );

        for (key, node) in [
            (21, Node::new(4, 2, 3)),
            (22, Node::new(7, 2, 2)),
            (23, Node::new(INF, 0, 1)),
            (21, Node::new(INF, 0, 2)),
            (20, Node::new(0, INF, 4)),
        ] {
            table.insert_key(key, node);
            assert_cached_matches_reference(
                &mut table,
                parent_key,
                &candidates,
                SelectionKind::Defence,
                4,
            );
        }

        let stats = table.selection_stats();
        assert_eq!(stats.full_scans, 1);
        assert_eq!(stats.cache_hits, 5);
        assert_eq!(stats.incremental_updates, 5);
        assert_eq!(stats.child_lookups, candidates.len() as u64);
    }

    #[test]
    fn potential_priority_biases_unknown_attack_but_switches_after_worsening() {
        let mut table = Table::new();
        let candidates = [(30, 9), (31, 5), (32, 3)];
        let parent_key = 300;

        let initial = table.select_cached(
            parent_key,
            candidates.len(),
            SelectionKind::Attack,
            6,
            candidates[0].1,
            || candidates.iter().copied(),
        );
        assert_eq!(initial.best_index, 0);
        assert_eq!(initial.next1, Node::new(3, 1, 6));
        assert_eq!(initial.next2, Node::new(7, 1, 6));

        table.insert_key(candidates[0].0, Node::new(8, 1, 6));
        let switched = table.select_cached(
            parent_key,
            candidates.len(),
            SelectionKind::Attack,
            6,
            candidates[0].1,
            || candidates.iter().copied(),
        );
        assert_eq!(switched.best_index, 1);
        assert_eq!(switched.next1.pn, 7);
    }

    #[test]
    fn potential_priority_biases_unknown_defence_and_stays_finite() {
        let mut table = Table::new();
        let candidates = [(40, u8::MAX), (41, 0), (42, 0)];
        let selected = table.select_cached(
            400,
            candidates.len(),
            SelectionKind::Defence,
            5,
            candidates[0].1,
            || candidates.iter().copied(),
        );

        assert_eq!(selected.best_index, 0);
        assert_eq!(selected.next1, Node::new(1, 3, 5));
        assert_eq!(selected.next2, Node::new(1, 9, 5));
        assert_ne!(selected.next2.dn, INF);
    }
}

pub const INF: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct Node {
    pub pn: u32,
    pub dn: u32,
    pub limit: u8,
}

impl Node {
    pub fn new(pn: u32, dn: u32, limit: u8) -> Self {
        Self {
            pn: pn,
            dn: dn,
            limit: limit,
        }
    }

    pub fn inf() -> Self {
        Self::new(INF, INF, 0)
    }

    pub fn zero_pn(limit: u8) -> Self {
        Self::new(0, INF, limit)
    }

    pub fn zero_dn(limit: u8) -> Self {
        Self::new(INF, 0, limit)
    }

    pub fn unit_pn(approx_dn: u32, limit: u8) -> Self {
        Self::new(1, approx_dn, limit)
    }

    pub fn unit_dn(approx_pn: u32, limit: u8) -> Self {
        Self::new(approx_pn, 1, limit)
    }

    pub fn proven(&self) -> bool {
        self.pn == 0
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let pn = if self.pn == INF {
            "INF".to_string()
        } else {
            self.pn.to_string()
        };
        let dn = if self.dn == INF {
            "INF".to_string()
        } else {
            self.dn.to_string()
        };
        write!(f, "(pn: {}, dn: {})", pn, dn)
    }
}
