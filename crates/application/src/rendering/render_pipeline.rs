use germinal_ports::{
    rendering::{
        frame_plan_builder::BuiltFramePlan, frame_plan_executor::FramePlanExecutor,
        render_target_id::RenderTargetId,
    },
    seq::Seq,
};

use crate::rendering::render_scheduler::{BuildRequest, BuildResult, ReadyFrame, RenderScheduler};

#[derive(Debug)]
pub struct RenderPipeline<E> {
    scheduler: RenderScheduler,
    pub(crate) executor: E,
}

impl<E> RenderPipeline<E>
where
    E: FramePlanExecutor,
{
    pub fn new(executor: E) -> Self {
        Self {
            scheduler: RenderScheduler::new(),
            executor,
        }
    }

    pub fn register_target(&mut self, target_id: RenderTargetId) {
        self.scheduler.register_target(target_id);
    }

    pub fn on_input_updated(&mut self, target_id: RenderTargetId, seq: Seq) -> InputUpdateResult {
        let Some(request) = self.scheduler.mark_input_updated(target_id, seq) else {
            return InputUpdateResult::NoTaskSubmitted;
        };

        self.submit(request);

        InputUpdateResult::TaskSubmitted(request)
    }

    pub fn on_frame_built(&mut self, frame: BuiltFramePlan) -> FrameBuiltResult {
        let result = self.scheduler.complete_build(frame.target_id, frame.seq);

        if let Some(next_request) = result.next_request {
            self.submit(next_request);
        }

        FrameBuiltResult { frame, result }
    }

    pub fn mark_presented(&mut self, target_id: RenderTargetId, seq: Seq) -> bool {
        self.scheduler.mark_presented(target_id, seq)
    }

    fn submit(&self, request: BuildRequest) {
        self.executor.submit(request.into_task());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputUpdateResult {
    NoTaskSubmitted,
    TaskSubmitted(BuildRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBuiltResult {
    pub frame: BuiltFramePlan,
    pub result: BuildResult,
}

impl FrameBuiltResult {
    pub fn ready_frame(&self) -> Option<ReadyFrame> {
        self.result.ready
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use germinal_ports::{
        rendering::{
            frame_plan_builder::{BuildFramePlanTask, BuiltFramePlan, RenderCommandDto},
            render_target_id::RenderTargetId,
        },
        seq::Seq,
    };

    use super::*;

    #[derive(Debug, Default)]
    struct TestFramePlanExecutor {
        submitted: RefCell<Vec<BuildFramePlanTask>>,
    }

    impl TestFramePlanExecutor {
        fn submitted(&self) -> Vec<BuildFramePlanTask> {
            self.submitted.borrow().clone()
        }
    }

    impl FramePlanExecutor for TestFramePlanExecutor {
        fn submit(&self, task: BuildFramePlanTask) {
            self.submitted.borrow_mut().push(task);
        }
    }

    fn target(value: u64) -> RenderTargetId {
        RenderTargetId::new(value)
    }

    fn frame(target_id: RenderTargetId, seq: Seq) -> BuiltFramePlan {
        BuiltFramePlan {
            target_id,
            seq,
            commands: vec![RenderCommandDto::Clear],
        }
    }

    #[test]
    fn input_update_submits_build_task() {
        let target_id = target(1);
        let executor = TestFramePlanExecutor::default();
        let mut pipeline = RenderPipeline::new(executor);

        pipeline.register_target(target_id);

        let result = pipeline.on_input_updated(target_id, Seq::new(1));

        assert_eq!(
            result,
            InputUpdateResult::TaskSubmitted(BuildRequest {
                target_id,
                seq: Seq::new(1)
            })
        );

        assert_eq!(
            pipeline.executor.submitted(),
            vec![BuildFramePlanTask {
                target_id,
                seq: Seq::new(1)
            }]
        );
    }

    #[test]
    fn input_during_in_flight_does_not_submit_extra_task() {
        let target_id = target(1);
        let executor = TestFramePlanExecutor::default();
        let mut pipeline = RenderPipeline::new(executor);

        pipeline.register_target(target_id);

        assert!(matches!(
            pipeline.on_input_updated(target_id, Seq::new(1)),
            InputUpdateResult::TaskSubmitted(_)
        ));

        assert_eq!(
            pipeline.on_input_updated(target_id, Seq::new(2)),
            InputUpdateResult::NoTaskSubmitted
        );

        assert_eq!(
            pipeline.on_input_updated(target_id, Seq::new(5)),
            InputUpdateResult::NoTaskSubmitted
        );

        assert_eq!(
            pipeline.executor.submitted(),
            vec![BuildFramePlanTask {
                target_id,
                seq: Seq::new(1)
            }]
        );
    }

    #[test]
    fn frame_built_with_newer_dirty_input_submits_latest_next_task() {
        let target_id = target(1);
        let executor = TestFramePlanExecutor::default();
        let mut pipeline = RenderPipeline::new(executor);

        pipeline.register_target(target_id);

        pipeline.on_input_updated(target_id, Seq::new(1));

        pipeline.on_input_updated(target_id, Seq::new(2));
        pipeline.on_input_updated(target_id, Seq::new(3));
        pipeline.on_input_updated(target_id, Seq::new(5));

        let built = pipeline.on_frame_built(frame(target_id, Seq::new(1)));

        assert_eq!(
            built.ready_frame(),
            Some(ReadyFrame {
                target_id,
                seq: Seq::new(1)
            })
        );

        assert_eq!(
            built.result.next_request,
            Some(BuildRequest {
                target_id,
                seq: Seq::new(5)
            })
        );

        assert_eq!(
            pipeline.executor.submitted(),
            vec![
                BuildFramePlanTask {
                    target_id,
                    seq: Seq::new(1)
                },
                BuildFramePlanTask {
                    target_id,
                    seq: Seq::new(5)
                },
            ]
        );
    }

    #[test]
    fn stale_frame_built_is_rejected() {
        let target_id = target(1);
        let executor = TestFramePlanExecutor::default();
        let mut pipeline = RenderPipeline::new(executor);

        pipeline.register_target(target_id);

        pipeline.on_input_updated(target_id, Seq::new(5));

        let built = pipeline.on_frame_built(frame(target_id, Seq::new(3)));

        assert!(!built.result.accepted);
        assert_eq!(built.result.ready, None);
        assert_eq!(built.result.next_request, None);

        assert_eq!(
            pipeline.executor.submitted(),
            vec![BuildFramePlanTask {
                target_id,
                seq: Seq::new(5)
            }]
        );
    }

    #[test]
    fn ready_frame_can_be_presented() {
        let target_id = target(1);
        let executor = TestFramePlanExecutor::default();
        let mut pipeline = RenderPipeline::new(executor);

        pipeline.register_target(target_id);

        pipeline.on_input_updated(target_id, Seq::new(1));

        let built = pipeline.on_frame_built(frame(target_id, Seq::new(1)));
        let ready = built.ready_frame().unwrap();

        assert!(pipeline.mark_presented(ready.target_id, ready.seq));
        assert!(!pipeline.mark_presented(ready.target_id, ready.seq));
    }
}
