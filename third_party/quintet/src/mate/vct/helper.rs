use super::state::VCTState;
use crate::mate::mate::Mate;
use crate::mate::state::State;
use crate::mate::vcf;

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
        let result = self.attacker_vcf_solver().solve(state);
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
        let result = self.attacker_vcf_solver().solve(state);
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
        let result = self.defender_vcf_solver().solve(state);
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
        let result = self.defender_vcf_solver().solve(state);
        self.poll_interrupt();
        result
    }
}
