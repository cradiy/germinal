use crate::rendering::frame_plan_builder::{BuildFramePlanTask, BuiltFramePlan};

pub trait FramePlanExecutor {
	fn submit(&self, task: BuildFramePlanTask);
}

pub trait FramePlanCompletionSink {
	fn complete(&self, frame: BuiltFramePlan);
}
