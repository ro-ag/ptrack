#![forbid(unsafe_code)]

mod menu_spec;

pub use menu_spec::{
    DesktopPlatform, MenuDispatch, MenuEntrySpec, MenuRole, MenuSpec, WindowSpec, menu_dispatch,
    menu_spec, window_spec,
};

#[cfg(test)]
mod menu_spec_test;
