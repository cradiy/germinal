use crate::rendering::frame_plan_builder::BuiltFramePlan;

pub trait FramePlanPresenter {
	fn present(&self, frame: &BuiltFramePlan);
}
