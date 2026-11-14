//! `eval` sites per reader unit, with static evaluation counts (`specs/unit-eval.md` §5.5).

use tungsten_ir::model::{ComputeUnit, EvalKey, Expression, Function, Ref, Statement};

use super::{Ctx, expr_children};
use crate::model::EvalSiteInfo;

/// `None` is unbounded.
type Count = Option<u64>;

fn mul(a: Count, n: u64) -> Count {
    a.map(|a| a.saturating_mul(n))
}

struct Walk<'c, 'a> {
    ctx: &'c Ctx<'a>,
    /// Key, sites, evaluations.
    out: Vec<(EvalKey, usize, Count)>,
    stack: Vec<Ref<Function>>,
    /// Functions whose sites are already counted.
    counted: Vec<Ref<Function>>,
}

impl Walk<'_, '_> {
    fn function(&mut self, f: Ref<Function>, m: Count) {
        if self.stack.contains(&f) {
            return;
        }
        let Some(func) = self.ctx.program.functions.try_get(f) else { return };
        // A helper's sites count once in the text, but run at every call.
        let count_sites = !self.counted.contains(&f);
        self.counted.push(f);
        self.stack.push(f);
        self.stmts(&func.body, m, count_sites);
        self.stack.pop();
    }

    fn stmts(&mut self, body: &[Statement], m: Count, sites: bool) {
        for s in body {
            match s {
                Statement::Assign { value: e, .. } | Statement::Return(e) => self.expr(e, m, sites),
                Statement::If { condition, then_branch, else_branch } => {
                    self.expr(condition, m, sites);
                    self.stmts(then_branch, m, sites);
                    self.stmts(else_branch, m, sites);
                }
                Statement::While { condition, body } => {
                    self.expr(condition, None, sites);
                    self.stmts(body, None, sites);
                }
                Statement::Repeat { count, body } => self.stmts(body, mul(m, *count as u64), sites),
                Statement::Break => {}
            }
        }
    }

    fn expr(&mut self, e: &Expression, m: Count, sites: bool) {
        if let Expression::Eval { unit, origin_scale, position_scale, .. } = e {
            let key = EvalKey::new(*unit, *origin_scale, *position_scale);
            let site = usize::from(sites);
            match self.out.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, s, c)) => {
                    *s += site;
                    *c = c.zip(m).map(|(a, b)| a.saturating_add(b));
                }
                None => self.out.push((key, site, m)),
            }
        }
        if let Expression::FunctionCall { function, .. } = e {
            self.function(*function, m);
        }
        for c in expr_children(e) {
            self.expr(c, m, sites);
        }
    }
}

/// The eval keys `cu`'s main function reaches through calls, in discovery order.
pub(crate) fn unit_evals(ctx: &Ctx, cu: Ref<ComputeUnit>) -> Vec<EvalSiteInfo> {
    let ComputeUnit::DensityFunction { function, .. } = &ctx.program.compute_units[cu];
    let mut w = Walk { ctx, out: vec![], stack: vec![], counted: vec![] };
    w.function(*function, Some(1));
    w.out
        .into_iter()
        .map(|(key, sites, evaluations)| EvalSiteInfo {
            unit: ctx.unit_id(key.unit),
            origin_scale: key.origin_scale(),
            position_scale: key.position_scale(),
            function: ctx.interface.eval_fns().iter().find(|e| e.key == key).map(|e| e.name.clone()),
            sites,
            evaluations,
        })
        .collect()
}
