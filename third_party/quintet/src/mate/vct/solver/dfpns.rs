use crate::board::Point;
use crate::mate::vcf;
use crate::mate::vct::generator::Generator;
use crate::mate::vct::helper::VCFHelper;
use crate::mate::vct::observer::SearchObserver;
use crate::mate::vct::proof::*;
use crate::mate::vct::resolver::Resolver;
use crate::mate::vct::searcher::Searcher;
use crate::mate::vct::selector::*;
use crate::mate::vct::solver::VCTSolver;
use crate::mate::vct::traverser::*;
use crate::mate::vct::{
    VCTSearchControl, VCTSearchProgress, VCTSearchResult, VCTSearchStop, VCTState,
};
use lru::LruCache;

pub struct DFPNSVCTSolver {
    attacker_table: Table,
    defender_table: Table,
    attacker_vcf_depth: u8,
    defender_vcf_depth: u8,
    attacker_vcf_solver: vcf::IDDFSSolver,
    defender_vcf_solver: vcf::IDDFSSolver,
    attacks_cache: LruCache<u64, Result<Vec<Point>, Node>>,
    defences_cache: LruCache<u64, Result<Vec<Point>, Node>>,
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
        if let Some(stopped) = self.stopped {
            return VCTSearchResult::Stopped(stopped);
        }
        if let Some(mate) = result.as_ref() {
            if let Some(control) = self.control.as_mut() {
                control.report_final(mate.path().clone(), self.visited_nodes);
            }
        }
        VCTSearchResult::Completed(result)
    }
}

impl VCTSolver for DFPNSVCTSolver {}

impl Searcher for DFPNSVCTSolver {}

impl Generator for DFPNSVCTSolver {
    fn attacks_cache(&mut self) -> &mut LruCache<u64, Result<Vec<Point>, Node>> {
        &mut self.attacks_cache
    }

    fn defences_cache(&mut self) -> &mut LruCache<u64, Result<Vec<Point>, Node>> {
        &mut self.defences_cache
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
