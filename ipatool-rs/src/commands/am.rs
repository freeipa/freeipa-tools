use anyhow::Result;

use super::Ctx;
use crate::patch::collect_patches;

pub fn run(ctx: &Ctx, patch_paths: &[String]) -> Result<()> {
    let patchdir = ctx.config.patchdir_expanded();
    let patches = collect_patches(
        patch_paths,
        &patchdir,
        &ctx.config.ticket_url,
        &ctx.config.legacy_ticket_url,
    )?;
    super::start_review::am_patches(ctx, &patches)
}
