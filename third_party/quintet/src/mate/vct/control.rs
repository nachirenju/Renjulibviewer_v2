use crate::board::Point;
use crate::mate::Mate;

/// Why a controlled VCT search stopped before a proof was completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VCTSearchStop {
    Cancelled,
    TimedOut,
}

/// A throttled snapshot of the current most-proving descent.
///
/// `path` is relative to the position passed to the solver.  It is an
/// in-progress line, not a proof until `VCTSearchResult::Completed(Some(_))`
/// has been returned.
#[derive(Clone, Debug)]
pub struct VCTSearchProgress {
    pub path: Vec<Point>,
    pub attacking: bool,
    pub pn: u32,
    pub dn: u32,
    pub visited_nodes: u64,
}

/// The only successful terminal result is `Completed(Some(Mate))`.  An
/// interruption never reuses an in-progress node as a proof/disproof.
#[derive(Debug)]
pub enum VCTSearchResult {
    Completed(Option<Mate>),
    Stopped(VCTSearchStop),
}

/// Callbacks used by the controlled DF-PNS entry point.
///
/// The stop callback is polled at every DF-PNS descent and around each VCF
/// probe.  The progress callback is throttled to avoid flooding a UI worker.
pub struct VCTSearchControl {
    should_stop: Box<dyn FnMut() -> Option<VCTSearchStop> + Send>,
    on_progress: Box<dyn FnMut(VCTSearchProgress) + Send>,
    /// Progress throttling deliberately uses a node cadence rather than a
    /// clock. `std::time::Instant` is not uniformly available inside every
    /// wasm Worker runtime, while the host adapter already owns the deadline.
    reports_since_last: u32,
    last_path: Vec<Point>,
}

impl VCTSearchControl {
    pub fn new<S, P>(should_stop: S, on_progress: P) -> Self
    where
        S: FnMut() -> Option<VCTSearchStop> + Send + 'static,
        P: FnMut(VCTSearchProgress) + Send + 'static,
    {
        Self {
            should_stop: Box::new(should_stop),
            on_progress: Box::new(on_progress),
            reports_since_last: 0,
            last_path: Vec::new(),
        }
    }

    pub fn poll_stop(&mut self) -> Option<VCTSearchStop> {
        (self.should_stop)()
    }

    pub fn report(&mut self, progress: VCTSearchProgress) {
        let path_extended = progress.path.len() > self.last_path.len()
            && progress.path.starts_with(&self.last_path);
        self.reports_since_last = self.reports_since_last.saturating_add(1);
        if !path_extended && self.reports_since_last < 64 {
            return;
        }
        self.last_path = progress.path.clone();
        self.reports_since_last = 0;
        (self.on_progress)(progress);
    }

    pub fn report_final(&mut self, path: Vec<Point>, visited_nodes: u64) {
        self.last_path = path.clone();
        self.reports_since_last = 0;
        (self.on_progress)(VCTSearchProgress {
            path,
            attacking: true,
            pn: 0,
            dn: u32::MAX,
            visited_nodes,
        });
    }
}
