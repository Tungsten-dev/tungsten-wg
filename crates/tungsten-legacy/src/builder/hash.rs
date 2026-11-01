use crate::spmt::model::*;
use std::hash::{DefaultHasher, Hash, Hasher};

type E<'m> = Expression<'m>;
type S<'m> = Statement<'m>;

// ============================================================================
// Stable structural hash
// ============================================================================
// `Debug` on `Interned` prints a pointer, so it can't be used for a cache key
// that must survive across runs. This walks the IR and hashes only content.

pub struct Fingerprint(pub DefaultHasher);

impl Fingerprint {
    pub fn tag(&mut self, t: u8) {
        self.0.write_u8(t);
    }
    pub fn var(&mut self, v: &Var<'_>) {
        format!("{:?}", v.name).hash(&mut self.0);
        format!("{:?}", v.t).hash(&mut self.0);
    }
    pub fn triple_f64(&mut self, t: (f64, f64, f64)) {
        for x in [t.0, t.1, t.2] {
            self.0.write_u64(x.to_bits());
        }
    }
    pub fn triple_i32(&mut self, t: (i32, i32, i32)) {
        for x in [t.0, t.1, t.2] {
            self.0.write_i32(x);
        }
    }
    pub fn density_input(&mut self, d: &DensityInput<'_>) {
        self.var(&d.var);
        self.0.write_u64(d.density_function.source_hash);
        self.triple_f64(d.scaled_origin);
        self.triple_f64(d.scaled_position);
        self.triple_i32(d.dimensions);
    }
    pub fn function(&mut self, f: &Function<'_>) {
        f.canonical_name.hash(&mut self.0);
        for p in &f.parameters {
            self.var(p);
        }
        for v in &f.variables {
            self.var(v);
        }
        format!("{:?}", f.return_type).hash(&mut self.0);
        self.stmts(&f.body);
    }

    pub fn stmts(&mut self, ss: &[Statement<'_>]) {
        self.0.write_usize(ss.len());
        for s in ss {
            self.stmt(s);
        }
    }

    pub fn stmt(&mut self, s: &Statement<'_>) {
        match s {
            S::Assign { target, value } => {
                self.tag(0);
                self.var(target);
                self.expr(value);
            }
            S::Return(e) => {
                self.tag(1);
                self.expr(e);
            }
            S::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.tag(2);
                self.expr(condition);
                self.stmts(then_branch);
                self.stmts(else_branch);
            }
            S::While { condition, body } => {
                self.tag(3);
                self.expr(condition);
                self.stmts(body);
            }
            S::Repeat { count, body } => {
                self.tag(4);
                self.0.write_usize(*count);
                self.stmts(body);
            }
            S::Break => self.tag(5),
        }
    }

    pub fn exprs(&mut self, es: &[Expression<'_>]) {
        self.0.write_usize(es.len());
        for e in es {
            self.expr(e);
        }
    }

    pub fn expr(&mut self, e: &Expression<'_>) {
        match e {
            E::Variable(v) => {
                self.tag(0);
                self.var(v);
            }
            E::Float(x) => {
                self.tag(1);
                self.0.write_u32(x.to_bits());
            }
            E::Double(x) => {
                self.tag(2);
                self.0.write_u64(x.to_bits());
            }
            E::Int(x) => {
                self.tag(3);
                self.0.write_i32(*x);
            }
            E::Long(x) => {
                self.tag(4);
                self.0.write_i64(*x);
            }
            E::FunctionCall {
                function,
                parameters,
            } => {
                self.tag(5);
                self.function(function); // by content, not by pointer
                self.exprs(parameters);
            }
            E::ExternCall {
                function_name,
                parameters,
                parameter_types,
                return_type,
            } => {
                self.tag(6);
                function_name.hash(&mut self.0);
                format!("{:?}", parameter_types).hash(&mut self.0);
                format!("{:?}", return_type).hash(&mut self.0);
                self.exprs(parameters);
            }
            E::DensityVariable(input, index) => {
                self.tag(7);
                self.density_input(input);
                match index {
                    Some(i) => {
                        self.tag(1);
                        self.expr(i);
                    }
                    None => self.tag(0),
                }
            }
            E::PermutationTable(p) => {
                self.tag(8);
                p.hash(&mut self.0);
            }
            E::BinaryOp { op, left, right } => {
                self.tag(9);
                self.tag(*op as u8);
                self.expr(left);
                self.expr(right);
            }
            E::UnaryOp { op, operand } => {
                self.tag(10);
                self.tag(*op as u8);
                self.expr(operand);
            }
            E::Field {
                base,
                field,
                type_of_field,
                known_idnex,
            } => {
                self.tag(11);
                self.expr(base);
                field.hash(&mut self.0);
                format!("{:?}", type_of_field).hash(&mut self.0);
                known_idnex.hash(&mut self.0);
            }
            E::ArrayAccess { array, index } => {
                self.tag(12);
                self.expr(array);
                self.expr(index);
            }
            E::Construct { t, args } => {
                self.tag(13);
                format!("{:?}", t).hash(&mut self.0);
                self.exprs(args);
            }
            E::ConstructExtern { t, args } => {
                self.tag(14);
                format!("{:?}", t).hash(&mut self.0);
                self.0.write_usize(args.len());
                for (name, a) in args {
                    name.hash(&mut self.0);
                    self.expr(a);
                }
            }
            E::ArrayLiteral(items) => {
                self.tag(15);
                self.exprs(items);
            }
            E::Bool(v) => {
                self.tag(16);
                v.hash(&mut self.0);
            }
            E::ExplicitCast { to, expr } => {
                self.tag(17);
                format!("{:?}", to).hash(&mut self.0);
                self.expr(expr);
            }
            E::Byte(x) => {
                self.tag(18);
                self.0.write_i8(*x);
            }
        }
    }
}
