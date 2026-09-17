use crate::board::Point;
use crate::mate::state::State;
use crate::mate::vct::generator::Candidate;
use crate::mate::vct::proof::*;
use crate::mate::vct::state::VCTState;

pub struct Selection {
    pub best: Option<Point>,
    pub current: Node,
    pub next1: Node,
    pub next2: Node,
}

impl Selection {
    pub fn interrupted() -> Self {
        Self {
            best: None,
            current: Node::inf(),
            next1: Node::inf(),
            next2: Node::inf(),
        }
    }
}

pub trait Selector: ProofTree {
    fn select_attack(&mut self, state: &mut VCTState, attacks: &[Candidate]) -> Selection {
        let limit = state.limit;
        let parent_key = state.zobrist_hash();
        let max_priority = attacks
            .first()
            .map(|candidate| candidate.priority)
            .unwrap_or(0);
        let cached = self.attacker_table().select_cached(
            parent_key,
            attacks.len(),
            SelectionKind::Attack,
            limit,
            max_priority,
            || {
                attacks
                    .iter()
                    .map(|candidate| (candidate.child_hash, candidate.priority))
            },
        );
        let best = attacks[cached.best_index].point;
        Selection {
            best: Some(best),
            current: cached.current,
            next1: cached.next1,
            next2: cached.next2,
        }
    }

    fn select_defence(&mut self, state: &mut VCTState, defences: &[Candidate]) -> Selection {
        let limit = state.limit;
        let child_limit = limit.saturating_sub(1);
        let parent_key = state.zobrist_hash();
        let max_priority = defences
            .first()
            .map(|candidate| candidate.priority)
            .unwrap_or(0);
        let cached = self.defender_table().select_cached(
            parent_key,
            defences.len(),
            SelectionKind::Defence,
            child_limit,
            max_priority,
            || {
                defences
                    .iter()
                    .map(|candidate| (candidate.child_hash, candidate.priority))
            },
        );
        let best = defences[cached.best_index].point;
        Selection {
            best: Some(best),
            current: cached.current,
            next1: cached.next1,
            next2: cached.next2,
        }
    }
}
