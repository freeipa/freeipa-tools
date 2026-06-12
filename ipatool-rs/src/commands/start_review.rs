use anyhow::{bail, Result};

use super::Ctx;

pub fn run(
    _ctx: &Ctx,
    _force: bool,
    _do_am: bool,
    _ticket_args: &[u64],
    _patch_paths: &[String],
) -> Result<()> {
    bail!("Setting reviewer via API is not yet implemented — this command is a work in progress");
}

pub fn am_patches(ctx: &Ctx, patches: &[crate::patch::Patch]) -> Result<()> {
    for patch in patches {
        println!("Applying patch: {}", patch.display_name());
        let content = patch.content();
        let am_command: Vec<&str> = ctx.config.am_command.iter().map(|s| s.as_str()).collect();
        if am_command.is_empty() {
            bail!("am-command is not configured");
        }
        let result = crate::git::run_process(
            &am_command,
            &ctx.git_env,
            Some(&content),
            true,
            Some(120),
            ctx.verbosity.max(2),
        )?;
        if result.returncode != 0 {
            bail!("am-command failed for patch: {}", patch.display_name());
        }
    }
    Ok(())
}
