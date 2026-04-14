//! `makakoo nursery hatch|list` — mascot registry ops.

use chrono::Utc;

use makakoo_core::nursery::{Mascot, MascotStatus, MascotVoice};

use crate::cli::NurseryCmd;
use crate::context::CliContext;
use crate::output;

pub fn run(ctx: &CliContext, cmd: NurseryCmd) -> anyhow::Result<i32> {
    match cmd {
        NurseryCmd::List => {
            let reg = ctx.nursery()?;
            output::print_mascot_list(&reg.all());
            Ok(0)
        }
        NurseryCmd::Hatch {
            name,
            species,
            maintainer,
            job,
        } => {
            let reg = ctx.nursery()?;
            let mascot = Mascot {
                name: name.clone(),
                species: species.clone(),
                maintainer,
                job,
                voice: MascotVoice {
                    greeting: format!("* {name} the {species} is ready to patrol"),
                    alert: format!("* {name}: something looks off!"),
                    success: format!("* {name}: patrol clean, all systems green"),
                    sleeping: format!("* {name} is resting..."),
                },
                patrol_interval_hours: 2,
                created_at: Utc::now(),
                status: MascotStatus::Hatching,
            };
            reg.register(mascot)?;
            output::print_info(format!(
                "hatched {} the {} — status=Hatching",
                name, species
            ));
            Ok(0)
        }
    }
}
