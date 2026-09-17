use super::state::VCTState;
use crate::mate::mate::Mate;
use crate::mate::state::State;
use crate::mate::vcf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VCFProbeKind {
    AttackerVcf,
    AttackerThreat,
    DefenderVcf,
    DefenderThreat,
}

pub trait VCFHelper {
    fn attacker_vcf_depth(&self) -> u8;
    fn defender_vcf_depth(&self) -> u8;

    fn attacker_vcf_solver(&mut self) -> &mut vcf::IDDFSSolver;
    fn defender_vcf_solver(&mut self) -> &mut vcf::IDDFSSolver;

    /// A VCF probe is a bounded sub-search of VCT.  An interrupted probe must
    /// not be cached as a negative VCF result; the outer VCT loop observes the
    /// stop condition immediately after it returns.
    fn poll_interrupt(&mut self) -> bool {
        false
    }

    fn lookup_positive_vcf(&mut self, _kind: VCFProbeKind, _key: u64) -> Option<Mate> {
        None
    }

    fn store_positive_vcf(&mut self, _kind: VCFProbeKind, _key: u64, _mate: Mate) {}

    fn solve_attacker_vcf(&mut self, state: &VCTState) -> Option<Mate> {
        if self.poll_interrupt() {
            return None;
        }
        if !state.attacking() {
            // A stale/cancelled DF-PNS branch must not abort a WASM worker.
            // Treat an invalid probe context as unavailable; callers never
            // use this as a positive VCF proof.
            debug_assert!(state.attacking());
            return None;
        }
        let state = &mut state.vcf_state(self.attacker_vcf_depth());
        let key = state.zobrist_hash();
        if let Some(mate) = self.lookup_positive_vcf(VCFProbeKind::AttackerVcf, key) {
            return Some(mate);
        }
        let result = self.attacker_vcf_solver().solve(state);
        if let Some(mate) = result.as_ref() {
            self.store_positive_vcf(VCFProbeKind::AttackerVcf, key, mate.clone());
        }
        self.poll_interrupt();
        result
    }

    fn solve_attacker_threat(&mut self, state: &VCTState) -> Option<Mate> {
        if self.poll_interrupt() {
            return None;
        }
        if state.attacking() {
            debug_assert!(!state.attacking());
            return None;
        }
        let state = &mut state.threat_state(self.attacker_vcf_depth());
        let key = state.zobrist_hash();
        if let Some(mate) = self.lookup_positive_vcf(VCFProbeKind::AttackerThreat, key) {
            return Some(mate);
        }
        let result = self.attacker_vcf_solver().solve(state);
        if let Some(mate) = result.as_ref() {
            self.store_positive_vcf(VCFProbeKind::AttackerThreat, key, mate.clone());
        }
        self.poll_interrupt();
        result
    }

    fn solve_defender_vcf(&mut self, state: &VCTState) -> Option<Mate> {
        if self.poll_interrupt() {
            return None;
        }
        if state.attacking() {
            debug_assert!(!state.attacking());
            return None;
        }
        let state = &mut state.vcf_state(self.defender_vcf_depth());
        let key = state.zobrist_hash();
        if let Some(mate) = self.lookup_positive_vcf(VCFProbeKind::DefenderVcf, key) {
            return Some(mate);
        }
        let result = self.defender_vcf_solver().solve(state);
        if let Some(mate) = result.as_ref() {
            self.store_positive_vcf(VCFProbeKind::DefenderVcf, key, mate.clone());
        }
        self.poll_interrupt();
        result
    }

    fn solve_defender_threat(&mut self, state: &VCTState) -> Option<Mate> {
        if self.poll_interrupt() {
            return None;
        }
        if !state.attacking() {
            debug_assert!(state.attacking());
            return None;
        }
        let state = &mut state.threat_state(self.defender_vcf_depth());
        let key = state.zobrist_hash();
        if let Some(mate) = self.lookup_positive_vcf(VCFProbeKind::DefenderThreat, key) {
            return Some(mate);
        }
        let result = self.defender_vcf_solver().solve(state);
        if let Some(mate) = result.as_ref() {
            self.store_positive_vcf(VCFProbeKind::DefenderThreat, key, mate.clone());
        }
        self.poll_interrupt();
        result
    }
}
