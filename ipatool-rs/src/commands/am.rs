use anyhow::Result;

use super::Ctx;
use crate::patch::collect_patches;

pub fn run(ctx: &Ctx, patch_paths: &[String]) -> Result<()> {
    let patchdir = ctx.config.patchdir_expanded();
    let ticket_url = ctx.config.ticket_url.clone();
    let patches = collect_patches(patch_paths, &patchdir, &ticket_url)?;
    super::start_review::am_patches(ctx, &patches)
}
