//! Headless format conversion, exposed by the `tabl convert` subcommand.

use std::path::Path;

use tabl_core::Result;

use crate::{format::Format, io, io::LoadOptions};

/// Load `input`, then write it back out as `output` in `out_fmt`.
pub fn convert(input: &Path, output: &Path, out_fmt: Format, opts: LoadOptions) -> Result<()> {
    let sheet = io::load_with(input, opts)?;
    io::save(&sheet, out_fmt, output)
}
