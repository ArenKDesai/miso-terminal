//! Every built-in function. To add one: create a module with a `SPEC` and a
//! `Panel`, then list it below. The command line, menus, HELP, layout
//! persistence and the smoke tests pick it up from this list.

mod asm;
mod capacity;
mod cons;
mod fuel;
mod gp;
mod help;
mod home;
mod lmp;
mod load;
mod log;
mod nsi;
mod outages;
mod renew;
mod theme;

use crate::function::FunctionSpec;

pub fn all() -> Vec<FunctionSpec> {
    vec![
        home::SPEC,
        lmp::SPEC,
        gp::SPEC,
        asm::SPEC,
        load::SPEC,
        capacity::SPEC,
        fuel::SPEC,
        renew::SPEC,
        nsi::SPEC,
        cons::SPEC,
        outages::SPEC,
        log::SPEC,
        theme::SPEC,
        help::SPEC,
    ]
}
