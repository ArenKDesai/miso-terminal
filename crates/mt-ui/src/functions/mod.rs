//! Every built-in function. To add one: create a module with a `SPEC` and a
//! `Panel`, then list it below. The command line, menus, HELP, layout
//! persistence and the smoke tests pick it up from this list.

mod acct;
mod ace;
mod act;
mod alrt;
mod asm;
mod bch;
mod capacity;
mod cn;
mod compare;
mod cons;
mod cross_chart;
mod dam;
mod des;
mod fuel;
mod gas;
pub(crate) mod gp;
mod help;
mod home;
mod hubs;
mod lmp;
mod load;
mod log;
mod map;
mod news;
mod ni;
mod nsi;
mod outages;
mod pnl;
mod port;
mod quote;
mod renew;
mod seam;
mod security_chart;
mod settings;
mod spread;
mod theme;
mod top;
mod transfer;
mod watchlist;
mod wx;

use crate::function::FunctionSpec;

pub fn all() -> Vec<FunctionSpec> {
    vec![
        home::SPEC,
        lmp::SPEC,
        hubs::SPEC,
        dam::SPEC,
        map::SPEC,
        gp::SPEC,
        spread::SPEC,
        compare::SPEC,
        seam::SPEC,
        watchlist::SPEC,
        asm::SPEC,
        load::SPEC,
        capacity::SPEC,
        fuel::SPEC,
        gas::SPEC,
        renew::SPEC,
        nsi::SPEC,
        transfer::SPEC,
        ace::SPEC,
        wx::SPEC,
        cons::SPEC,
        bch::SPEC,
        outages::SPEC,
        quote::SPEC,
        des::SPEC,
        top::SPEC,
        news::SPEC,
        ni::SPEC,
        cn::SPEC,
        port::SPEC,
        acct::SPEC,
        pnl::SPEC,
        act::SPEC,
        alrt::SPEC,
        log::SPEC,
        theme::SPEC,
        settings::SPEC,
        help::SPEC,
    ]
}
