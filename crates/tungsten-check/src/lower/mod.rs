//! The lowering walker. Extern and record names resolve against the [`Registry`].

mod consts;
mod expr;
mod program;
mod stmt;
pub(crate) mod types;

use std::collections::HashMap;

use tungsten_core::Type;
use tungsten_ir::{Arena, Function, OptimizedProgram, Ref, Var, VariableType};

use crate::error::{Item, Location, LowerError, LowerErrorKind, Step};
use crate::map::LowerMap;
use crate::registry::Registry;
use types::{TypeFail, lower_type};

pub(crate) use consts::const_to_expr;
pub(crate) use program::run;

/// `Arena::get` without the panics: `contains` compares tags without asserting.
pub(crate) fn get<T>(a: &Arena<T>, r: Ref<T>) -> Option<&T> {
    if a.contains(r) { a.try_get(r) } else { None }
}

/// Lowering state: the program, the registry, the map built so far, collected errors and the
/// current location.
pub(crate) struct Ctx<'p, 'r> {
    pub p: &'p OptimizedProgram,
    pub reg: &'r Registry,
    pub map: LowerMap,
    pub errors: Vec<LowerError>,
    pub item: Item,
    pub path: Vec<Step>,
    /// The function being lowered, and its locals.
    pub func: Option<&'p Function>,
    pub locals: HashMap<Ref<Var>, tungsten_core::LocalId>,
}

impl<'p, 'r> Ctx<'p, 'r> {
    pub(crate) fn location(&self) -> Location {
        Location {
            item: self.item.clone(),
            path: self.path.clone(),
        }
    }

    pub(crate) fn err(&mut self, kind: LowerErrorKind) {
        let at = self.location();
        self.errors.push(LowerError { kind, at });
    }

    /// Runs `f` with `step` appended to the path.
    pub(crate) fn at<T>(&mut self, step: Step, f: impl FnOnce(&mut Self) -> T) -> T {
        self.path.push(step);
        let out = f(self);
        self.path.pop();
        out
    }

    /// Starts a new item with an empty path.
    pub(crate) fn enter(&mut self, item: Item) {
        self.item = item;
        self.path.clear();
    }

    /// Lowers a type at the current location. Failures become errors and a placeholder.
    pub(crate) fn ty(&mut self, t: &VariableType) -> Type {
        let reg = self.reg;
        match lower_type(t, &mut |n| reg.record_id(n)) {
            Ok(t) => t,
            Err(TypeFail::DensityInput) => {
                self.err(LowerErrorKind::DensityInputType);
                Type::Bool
            }
            Err(TypeFail::UnknownRecord(n)) => {
                self.err(LowerErrorKind::UnknownRecord(n.to_string()));
                Type::Bool
            }
        }
    }
}
