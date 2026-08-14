use germinal_ports::seq::Seq;

#[derive(Debug, Clone)]
pub struct RenderGenerationState {
    latest_input_seq: Seq,
    in_flight_seq: Option<Seq>,
    ready_seq: Option<Seq>,
    presented_seq: Option<Seq>,
    dirty: bool,
}

impl RenderGenerationState {
    pub fn new(initial_seq: Seq) -> Self {
        Self {
            latest_input_seq: initial_seq,
            in_flight_seq: None,
            ready_seq: None,
            presented_seq: None,
            dirty: false,
        }
    }

    pub fn latest_input_seq(&self) -> Seq {
        self.latest_input_seq
    }

    pub fn in_flight_seq(&self) -> Option<Seq> {
        self.in_flight_seq
    }

    pub fn ready_seq(&self) -> Option<Seq> {
        self.ready_seq
    }

    pub fn presented_seq(&self) -> Option<Seq> {
        self.presented_seq
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_input_updated(&mut self, seq: Seq) {
        if seq > self.latest_input_seq {
            self.latest_input_seq = seq;
            self.dirty = true;
        }
    }

    pub fn can_start_build(&self) -> bool {
        self.dirty && self.in_flight_seq.is_none()
    }

    pub fn start_build(&mut self) -> Option<Seq> {
        if !self.can_start_build() {
            return None;
        }

        let seq = self.latest_input_seq;
        self.in_flight_seq = Some(seq);
        self.dirty = false;
        Some(seq)
    }

    pub fn complete_build(&mut self, seq: Seq) -> BuildCompletion {
        if self.in_flight_seq != Some(seq) {
            return BuildCompletion::Stale;
        }

        self.in_flight_seq = None;

        if self.ready_seq.map_or_else(|| true, |ready| seq > ready) {
            self.ready_seq = Some(seq);

            if self.dirty {
                BuildCompletion::ReadyAndNeedsRebuild
            } else {
                BuildCompletion::Ready
            }
        } else {
            BuildCompletion::Stale
        }
    }

    pub fn mark_presented(&mut self, seq: Seq) -> bool {
        if self.ready_seq != Some(seq) {
            return false;
        }

        if self
            .presented_seq
            .map_or_else(|| false, |presented| seq <= presented)
        {
            return false;
        }

        self.presented_seq = Some(seq);
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildCompletion {
    Ready,
    ReadyAndNeedsRebuild,
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_build_only_when_dirty_and_not_in_flight() {
        let mut state = RenderGenerationState::new(Seq::ZERO);

        assert_eq!(state.start_build(), None);

        state.mark_input_updated(Seq::new(1));

        assert_eq!(state.start_build(), Some(Seq::new(1)));
        assert_eq!(state.in_flight_seq(), Some(Seq::new(1)));
        assert!(!state.is_dirty());

        assert_eq!(state.start_build(), None);
    }

    #[test]
    fn input_updates_during_in_flight_only_mark_dirty() {
        let mut state = RenderGenerationState::new(Seq::ZERO);

        state.mark_input_updated(Seq::new(1));
        assert_eq!(state.start_build(), Some(Seq::new(1)));

        state.mark_input_updated(Seq::new(2));
        state.mark_input_updated(Seq::new(3));
        state.mark_input_updated(Seq::new(5));

        assert_eq!(state.latest_input_seq(), Seq::new(5));
        assert_eq!(state.in_flight_seq(), Some(Seq::new(1)));
        assert!(state.is_dirty());
        assert_eq!(state.start_build(), None);
    }

    #[test]
    fn completing_build_reports_need_rebuild_when_newer_input_arrived() {
        let mut state = RenderGenerationState::new(Seq::ZERO);

        state.mark_input_updated(Seq::new(1));
        assert_eq!(state.start_build(), Some(Seq::new(1)));

        state.mark_input_updated(Seq::new(5));

        assert_eq!(
            state.complete_build(Seq::new(1)),
            BuildCompletion::ReadyAndNeedsRebuild
        );

        assert_eq!(state.ready_seq(), Some(Seq::new(1)));
        assert_eq!(state.in_flight_seq(), None);
        assert!(state.can_start_build());

        assert_eq!(state.start_build(), Some(Seq::new(5)));
    }

    #[test]
    fn completing_build_without_newer_input_becomes_ready() {
        let mut state = RenderGenerationState::new(Seq::ZERO);

        state.mark_input_updated(Seq::new(1));
        assert_eq!(state.start_build(), Some(Seq::new(1)));

        assert_eq!(state.complete_build(Seq::new(1)), BuildCompletion::Ready);
        assert_eq!(state.ready_seq(), Some(Seq::new(1)));
        assert!(!state.can_start_build());
    }

    #[test]
    fn stale_completion_is_rejected() {
        let mut state = RenderGenerationState::new(Seq::ZERO);

        state.mark_input_updated(Seq::new(5));
        assert_eq!(state.start_build(), Some(Seq::new(5)));

        assert_eq!(state.complete_build(Seq::new(3)), BuildCompletion::Stale);
        assert_eq!(state.ready_seq(), None);
        assert_eq!(state.in_flight_seq(), Some(Seq::new(5)));
    }

    #[test]
    fn only_ready_seq_can_be_presented() {
        let mut state = RenderGenerationState::new(Seq::ZERO);

        assert!(!state.mark_presented(Seq::new(1)));

        state.mark_input_updated(Seq::new(1));
        assert_eq!(state.start_build(), Some(Seq::new(1)));
        assert_eq!(state.complete_build(Seq::new(1)), BuildCompletion::Ready);

        assert!(state.mark_presented(Seq::new(1)));
        assert_eq!(state.presented_seq(), Some(Seq::new(1)));

        assert!(!state.mark_presented(Seq::new(1)));
        assert!(!state.mark_presented(Seq::new(0)));
    }
}
