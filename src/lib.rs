//! inkline: zsh-style autosuggestions and syntax highlighting for bash.
//!
//! A bash loadable builtin (`enable -f libinkline.so inkline`) that keeps GNU
//! readline as the line editor and only decorates its output. See `glue` for
//! the readline integration and `paint` for the display model.

pub mod highlight;
pub mod layout;
pub mod paint;
pub mod suggest;

#[cfg(not(test))]
mod glue;
