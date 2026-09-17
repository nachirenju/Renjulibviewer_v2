use crate::mate::vcf;
use crate::mate::vct::generator::{CandidateResult, Generator};
use crate::mate::vct::helper::{VCFHelper, VCFProbeKind};
use crate::mate::vct::observer::SearchObserver;
use crate::mate::vct::proof::*;
use crate::mate::vct::resolver::Resolver;
use crate::mate::vct::searcher::Searcher;
use crate::mate::vct::selector::*;
use crate::mate::vct::solver::VCTSolver;
use crate::mate::vct::traverser::*;
use crate::mate::vct::{
    VCTSearchControl, VCTSearchProgress, VCTSearchResult, VCTSearchStats, VCTSearchStop, VCTState,
};
use crate::mate::Mate;
use lru::LruCache;
use rustc_hash::FxHashMap;

pub struct DFPNSVCTSolver {
    attacker_table: Table,
    defender_table: Table,
    attacker_vcf_depth: u8,
    defender_vcf_depth: u8,
    attacker_vcf_solver: vcf::IDDFSSolver,
    defender_vcf_solver: vcf::IDDFSSolver,
    attacks_cache: LruCache<u64, CandidateResult>,
    defences_cache: LruCache<u64, CandidateResult>,
    positive_vcf_cache: FxHashMap<(VCFProbeKind, u64), Mate>,
    stats: VCTSearchStats,
    control: Option<VCTSearchControl>,
    stopped: Option<VCTSearchStop>,
    visited_nodes: u64,
}

impl DFPNSVCTSolver {
    pub fn init(attacker_vcf_depth: u8, defender_vcf_depth: u8) -> Self {
        Self {
            attacker_table: Table::new(),
            defender_table: Table::new(),
            attacker_vcf_depth: attacker_vcf_depth,
            defender_vcf_depth: defender_vcf_depth,
            attacker_vcf_solver: vcf::IDDFSSolver::init([1].to_vec()),
            defender_vcf_solver: vcf::IDDFSSolver::init([1].to_vec()),
            attacks_cache: LruCache::new(1000),
            defences_cache: LruCache::new(1000),
            positive_vcf_cache: FxHashMap::default(),
            stats: VCTSearchStats::default(),
            control: None,
            stopped: None,
            visited_nodes: 0,
        }
    }

    pub fn init_with_control(
        attacker_vcf_depth: u8,
        defender_vcf_depth: u8,
        control: VCTSearchControl,
    ) -> Self {
        let mut solver = Self::init(attacker_vcf_depth, defender_vcf_depth);
        solver.control = Some(control);
        solver
    }

    pub fn solve_with_control(&mut self, state: &mut VCTState) -> VCTSearchResult {
        let result = VCTSolver::solve(self, state);
        self.poll_stop();
        self.capture_selection_stats();
        if let Some(stopped) = self.stopped {
            return VCTSearchResult::Stopped(stopped, self.stats);
        }
        if let Some(mate) = result.as_ref() {
            if let Some(control) = self.control.as_mut() {
                control.report_final(mate.path().clone(), self.visited_nodes);
            }
        }
        VCTSearchResult::Completed(result, self.stats)
    }

    fn capture_selection_stats(&mut self) {
        let attack = self.attacker_table.selection_stats();
        self.stats.attack_selector_full_scans = attack.full_scans;
        self.stats.attack_selector_cache_hits = attack.cache_hits;
        self.stats.attack_selector_incremental_updates = attack.incremental_updates;
        self.stats.attack_selector_child_lookups = attack.child_lookups;

        let defence = self.defender_table.selection_stats();
        self.stats.defence_selector_full_scans = defence.full_scans;
        self.stats.defence_selector_cache_hits = defence.cache_hits;
        self.stats.defence_selector_incremental_updates = defence.incremental_updates;
        self.stats.defence_selector_child_lookups = defence.child_lookups;
    }
}

impl VCTSolver for DFPNSVCTSolver {}

impl Searcher for DFPNSVCTSolver {}

impl Generator for DFPNSVCTSolver {
    fn attacks_cache(&mut self) -> &mut LruCache<u64, CandidateResult> {
        &mut self.attacks_cache
    }

    fn defences_cache(&mut self) -> &mut LruCache<u64, CandidateResult> {
        &mut self.defences_cache
    }

    fn record_candidate_counts(&mut self, attacking: bool, before: usize, after: usize) {
        self.stats.record_candidates(attacking, before, after);
    }
}

impl VCFHelper for DFPNSVCTSolver {
    fn attacker_vcf_depth(&self) -> u8 {
        self.attacker_vcf_depth
    }

    fn defender_vcf_depth(&self) -> u8 {
        self.defender_vcf_depth
    }

    fn attacker_vcf_solver(&mut self) -> &mut vcf::IDDFSSolver {
        &mut self.attacker_vcf_solver
    }

    fn defender_vcf_solver(&mut self) -> &mut vcf::IDDFSSolver {
        &mut self.defender_vcf_solver
    }

    fn poll_interrupt(&mut self) -> bool {
        SearchObserver::poll_stop(self)
    }

    fn lookup_positive_vcf(&mut self, kind: VCFProbeKind, key: u64) -> Option<Mate> {
        let result = self.positive_vcf_cache.get(&(kind, key)).cloned();
        if result.is_some() {
            self.stats.positive_vcf_cache_hits =
                self.stats.positive_vcf_cache_hits.saturating_add(1);
        }
        result
    }

    fn store_positive_vcf(&mut self, kind: VCFProbeKind, key: u64, mate: Mate) {
        self.positive_vcf_cache.insert((kind, key), mate);
    }
}

impl SearchObserver for DFPNSVCTSolver {
    fn poll_stop(&mut self) -> bool {
        if self.stopped.is_some() {
            return true;
        }
        if let Some(control) = self.control.as_mut() {
            if let Some(stopped) = control.poll_stop() {
                self.stopped = Some(stopped);
                return true;
            }
        }
        false
    }

    fn report_selection(&mut self, state: &VCTState, selection: &Selection, attacking: bool) {
        if self.poll_stop() {
            return;
        }
        self.visited_nodes = self.visited_nodes.saturating_add(1);
        let progress = VCTSearchProgress {
            path: state.progress_path(selection.best),
            attacking,
            pn: selection.current.pn,
            dn: selection.current.dn,
            visited_nodes: self.visited_nodes,
        };
        if let Some(control) = self.control.as_mut() {
            control.report(progress);
        }
    }
}

impl Traverser for DFPNSVCTSolver {
    fn next_threshold_attack(&self, selection: &Selection, threshold: Node) -> Node {
        DFPNSTraverser::next_threshold_attack(self, selection, threshold)
    }

    fn next_threshold_defence(&self, selection: &Selection, threshold: Node) -> Node {
        DFPNSTraverser::next_threshold_defence(self, selection, threshold)
    }
}

impl DFPNSTraverser for DFPNSVCTSolver {}

impl Selector for DFPNSVCTSolver {}

impl ProofTree for DFPNSVCTSolver {
    fn attacker_table(&mut self) -> &mut Table {
        &mut self.attacker_table
    }

    fn defender_table(&mut self) -> &mut Table {
        &mut self.defender_table
    }
}

impl Resolver for DFPNSVCTSolver {}
