use crate::board::Point;
use crate::mate::state::State;
use crate::mate::vct::helper::VCFHelper;
use crate::mate::vct::proof::*;
use crate::mate::vct::state::VCTState;
use lru::LruCache;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub point: Point,
    pub child_hash: u64,
    /// Existing potential score used only as a finite DF-PN ordering prior.
    /// It never removes a candidate or changes a terminal proof value.
    pub priority: u8,
}

impl Candidate {
    pub fn new(state: &VCTState, point: Point) -> Self {
        Self::with_priority(state, point, 0)
    }

    pub fn with_priority(state: &VCTState, point: Point, priority: u8) -> Self {
        Self {
            point,
            child_hash: state.next_zobrist_hash(Some(point)),
            priority,
        }
    }
}

pub type Candidates = Arc<[Candidate]>;
pub type CandidateResult = Result<Candidates, Node>;

pub trait Generator: VCFHelper {
    fn attacks_cache(&mut self) -> &mut LruCache<u64, CandidateResult>;
    fn defences_cache(&mut self) -> &mut LruCache<u64, CandidateResult>;

    fn record_candidate_counts(&mut self, _attacking: bool, _before: usize, _after: usize) {}

    fn generate_attacks(&mut self, state: &mut VCTState) -> CandidateResult {
        let key = state.zobrist_hash();
        if let Some(hit) = self.attacks_cache().get(&key) {
            hit.clone()
        } else {
            let result = self.compute_attacks(state);
            self.attacks_cache().put(key, result.clone());
            result
        }
    }

    fn compute_attacks(&mut self, state: &mut VCTState) -> CandidateResult {
        // This is not necessary but improves speed
        if self.solve_attacker_vcf(state).is_some() {
            return Err(Node::zero_pn(state.limit));
        }

        // This is not necessary but narrows candidates
        let maybe_threat = self.solve_defender_threat(state);
        let maybe_threat_defences = maybe_threat.map(|t| state.threat_defences(&t));
        let candidates: Vec<Candidate> = if let Some(threat_defences) = maybe_threat_defences {
            let (mut scored, before_dedup) =
                state.sorted_potentials_counted(3, Some(threat_defences));
            self.record_candidate_counts(true, before_dedup, scored.len());
            scored.retain(|&x| !state.is_forbidden_move(x.0));
            scored
                .into_iter()
                .map(|(point, priority)| Candidate::with_priority(state, point, priority))
                .collect()
        } else {
            // The unrestricted path is already unique and correctly ordered in
            // PotentialField's buckets. Build Candidate values directly so we
            // do not allocate an intermediate scored-point Vec.
            let candidate_count = state.sorted_potential_count(3);
            self.record_candidate_counts(true, candidate_count, candidate_count);
            state
                .sorted_potential_iter(3)
                .filter(|&(point, _)| !state.is_forbidden_move(point))
                .map(|(point, priority)| Candidate::with_priority(state, point, priority))
                .collect()
        };

        if candidates.is_empty() {
            return Err(Node::zero_dn(state.limit));
        }

        Ok(Arc::from(candidates))
    }

    fn generate_defences(&mut self, state: &mut VCTState) -> CandidateResult {
        let key = state.zobrist_hash();
        if let Some(hit) = self.defences_cache().get(&key) {
            hit.clone()
        } else {
            let result = self.compute_defences(state);
            self.defences_cache().put(key, result.clone());
            result
        }
    }

    fn compute_defences(&mut self, state: &mut VCTState) -> CandidateResult {
        let maybe_threat = self.solve_attacker_threat(state);
        if maybe_threat.is_none() {
            return Err(Node::zero_dn(state.limit));
        }

        // This is not necessary but improves speed
        if self.solve_defender_vcf(state).is_some() {
            return Err(Node::zero_dn(state.limit));
        }

        let threat = maybe_threat.unwrap();
        let (mut result, before_dedup) =
            state.sort_by_potential_counted(state.threat_defences(&threat));
        self.record_candidate_counts(false, before_dedup, result.len());
        result.retain(|&x| !state.is_forbidden_move(x.0));

        if result.is_empty() {
            return Err(Node::zero_pn(state.limit));
        }

        let result: Vec<Candidate> = result
            .into_iter()
            .map(|(point, priority)| Candidate::with_priority(state, point, priority))
            .collect();
        Ok(Arc::from(result))
    }
}
