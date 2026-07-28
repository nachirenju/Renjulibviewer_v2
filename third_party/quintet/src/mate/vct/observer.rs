use super::selector::Selection;
use super::state::VCTState;

/// Search-loop hooks.  The default implementation deliberately does nothing
/// so the original DFS/PNS solvers retain their existing behaviour.
pub trait SearchObserver {
    fn poll_stop(&mut self) -> bool {
        false
    }

    fn report_selection(&mut self, _state: &VCTState, _selection: &Selection, _attacking: bool) {}
}
