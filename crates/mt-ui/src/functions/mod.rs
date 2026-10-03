//! Every built-in function. To add one: create a module with a `SPEC` and a
//! `Panel`, then list it below. The command line, menus, HELP, layout
//! persistence and the smoke tests pick it up from this list.

mod ace;
mod alrt;
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
mod map;
mod nsi;
mod outages;
mod renew;
mod spread;
mod theme;
mod transfer;
mod watchlist;

use crate::function::FunctionSpec;

pub fn all() -> Vec<FunctionSpec> {
    vec![
        home::SPEC,
        lmp::SPEC,
        map::SPEC,
        gp::SPEC,
        spread::SPEC,
        watchlist::SPEC,
        asm::SPEC,
        load::SPEC,
        capacity::SPEC,
        fuel::SPEC,
        renew::SPEC,
        nsi::SPEC,
        transfer::SPEC,
        ace::SPEC,
        cons::SPEC,
        outages::SPEC,
        alrt::SPEC,
        log::SPEC,
        theme::SPEC,
        help::SPEC,
    ]
}
