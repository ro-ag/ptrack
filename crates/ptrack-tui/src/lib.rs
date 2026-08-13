//! Pure reducer/render terminal UI and its small terminal runtime adapter.

mod input;
mod model;
mod reducer;
mod render;
mod runtime;

pub use input::{InputEditor, Key};
pub use model::{Effect, Model, RuntimeContext, Tab};
pub use reducer::update;
pub use render::draw;
pub use runtime::run;

#[cfg(test)]
mod input_test;
#[cfg(test)]
mod reducer_test;
#[cfg(test)]
mod render_test;
#[cfg(test)]
mod runtime_test;
