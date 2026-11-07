//! One test per expression rule (and `ConstExpr`), with the expected core `Expr`.

use tungsten_core::*;

use super::*;

fn b(e: Expr) -> Box<Expr> {
    Box::new(e)
}

fn p() -> Expr {
    Expr::Local(LocalId(0))
}

fn f64_lit(v: f64) -> Expr {
    Expr::Lit(Lit::F64(v.to_bits()))
}

fn i32_lit(v: i32) -> Expr {
    Expr::Lit(Lit::I32(v))
}

#[test]
fn literals() {
    assert_eq!(ret("return 1.5;"), f64_lit(1.5));
    assert_eq!(ret("return 0.1f;"), Expr::Lit(Lit::F32(0.1f32.to_bits())));
    assert_eq!(ret("return -3b;"), Expr::Lit(Lit::I8(-3)));
    assert_eq!(ret("return 7;"), i32_lit(7));
    assert_eq!(ret("return 1L;"), Expr::Lit(Lit::I64(1)));
    assert_eq!(ret("return true;"), Expr::Lit(Lit::Bool(true)));
    // NaN payloads and the sign of zero survive.
    assert_eq!(
        ret("return nan(0x7ff8000000000001);"),
        Expr::Lit(Lit::F64(0x7ff8000000000001))
    );
    assert_eq!(ret("return -0.0;"), Expr::Lit(Lit::F64((-0.0f64).to_bits())));
    assert_ne!(ret("return -0.0;"), ret("return 0.0;"));
}

#[test]
fn references() {
    assert_eq!(ret("return p;"), p());
    assert_eq!(ret("return const(c);"), Expr::Const(ConstId(0)));
    assert_eq!(ret("return host(h);"), Expr::Host(HostId(0)));
    assert_eq!(ret("return table(t);"), Expr::Table(TableId(0)));
    assert_eq!(ret("return ambient(origin);"), Expr::Ambient(Ambient::Origin));
    assert_eq!(ret("return ambient(pos3);"), Expr::Ambient(Ambient::Pos3));
    assert_eq!(
        ret("return ambient(position_scale);"),
        Expr::Ambient(Ambient::PositionScale)
    );
}

#[test]
fn density() {
    assert_eq!(
        ret("return density(d);"),
        Expr::Density {
            input: InputId(0),
            index: None
        }
    );
    assert_eq!(
        ret("return density(d, 2);"),
        Expr::Density {
            input: InputId(0),
            index: Some(b(i32_lit(2)))
        }
    );
}

#[test]
fn calls() {
    let r = registry();
    let l = ok(&load(&snippet("", "return leaf();")), &r);
    assert_eq!(
        *main_ret(&l),
        Expr::Call {
            func: fn_id(&l, "leaf"),
            args: vec![]
        }
    );
    // Annotations are lowered as written, even when they disagree with the registry.
    assert_eq!(
        ret("return ext sin::<(i32) -> f32>(p);"),
        Expr::ExternCall {
            ext: ExternId(0),
            params: vec![Type::I32],
            ret: Type::F32,
            args: vec![p()]
        }
    );
    assert_eq!(
        ret("return ext sample(table(t), p);"),
        Expr::ExternCall {
            ext: ExternId(1),
            params: vec![Type::PermTable, Type::F64],
            ret: Type::F64,
            args: vec![Expr::Table(TableId(0)), p()]
        }
    );
}

#[test]
fn operators() {
    assert_eq!(
        ret("return (p * 2.0);"),
        Expr::Binary {
            op: BinOp::Multiply,
            left: b(p()),
            right: b(f64_lit(2.0))
        }
    );
    assert_eq!(
        ret("return (1L >>> 2);"),
        Expr::Binary {
            op: BinOp::LogicalShiftRight,
            left: b(Expr::Lit(Lit::I64(1))),
            right: b(i32_lit(2))
        }
    );
    assert_eq!(
        ret("return -p;"),
        Expr::Unary {
            op: UnOp::Negate,
            operand: b(p())
        }
    );
    assert_eq!(
        ret("return !true;"),
        Expr::Unary {
            op: UnOp::Not,
            operand: b(Expr::Lit(Lit::Bool(true)))
        }
    );
    // `Ref` is the identity.
    assert_eq!(ret("return &p;"), p());
    assert_eq!(ret("return &mut p;"), p());
}

#[test]
fn fields() {
    assert_eq!(
        ret("return ambient(origin).y;"),
        Expr::Field {
            base: b(Expr::Ambient(Ambient::Origin)),
            sel: FieldSel::Vec3(Axis::Y),
            ty: Type::F64
        }
    );
    assert_eq!(
        ret("return ambient(pos3).z;"),
        Expr::Field {
            base: b(Expr::Ambient(Ambient::Pos3)),
            sel: FieldSel::Pos3(Axis::Z),
            ty: Type::I32
        }
    );
    // The selector follows the stored base type, even when it disagrees with the base. Lowering
    // doesn't validate it; the checker rejects the mismatch.
    assert_eq!(
        ret("return ambient(origin).x::<f64; pos3>;"),
        Expr::Field {
            base: b(Expr::Ambient(Ambient::Origin)),
            sel: FieldSel::Pos3(Axis::X),
            ty: Type::F64
        }
    );
    // The annotation is kept as written; the selector comes from the base.
    assert_eq!(
        ret("return new extern Pair(1.0, 2).b::<f32>;"),
        Expr::Field {
            base: b(Expr::MakeRecord {
                record: RecordId(0),
                fields: vec![
                    FieldInit {
                        field: 0,
                        value: f64_lit(1.0)
                    },
                    FieldInit {
                        field: 1,
                        value: i32_lit(2)
                    },
                ]
            }),
            sel: FieldSel::Record(RecordId(0), 1),
            ty: Type::F32
        }
    );
}

#[test]
fn index_and_arrays() {
    assert_eq!(
        ret("return [1.0, 2.0][0];"),
        Expr::Index {
            array: b(Expr::ArrayLit(vec![f64_lit(1.0), f64_lit(2.0)])),
            index: b(i32_lit(0))
        }
    );
    // Empty array literals are representable; the checker rejects them.
    assert_eq!(
        ret("let a: [f64; 0] = []; return p;"),
        p(),
    );
    let r = registry();
    let l = ok(
        &load(&snippet("", "let a: [f64; 0] = []; return p;")),
        &r,
    );
    let main = &l.program.functions[fn_id(&l, "top").0];
    assert_eq!(
        main.body[0],
        Stmt::Assign {
            target: LocalId(1),
            value: Expr::ArrayLit(vec![])
        }
    );
    assert_eq!(main.locals[1], Type::Array(Box::new(Type::F64), 0));
}

#[test]
fn constructs() {
    assert_eq!(
        ret("return vec3(1.0, 2.0, p);"),
        Expr::MakeVec3 {
            x: b(f64_lit(1.0)),
            y: b(f64_lit(2.0)),
            z: b(p())
        }
    );
    assert_eq!(
        ret("return pos3(1, 2, 3);"),
        Expr::MakePos3 {
            x: b(i32_lit(1)),
            y: b(i32_lit(2)),
            z: b(i32_lit(3))
        }
    );
    // Positional with too few args: representable, the checker rejects it.
    assert_eq!(
        ret("return new extern Pair(1.0);"),
        Expr::MakeRecord {
            record: RecordId(0),
            fields: vec![FieldInit {
                field: 0,
                value: f64_lit(1.0)
            }]
        }
    );
    // Named: written order, duplicates kept.
    assert_eq!(
        ret("return new extern Pair { b: 2, a: 1.0, b: 3 };"),
        Expr::MakeRecord {
            record: RecordId(0),
            fields: vec![
                FieldInit {
                    field: 1,
                    value: i32_lit(2)
                },
                FieldInit {
                    field: 0,
                    value: f64_lit(1.0)
                },
                FieldInit {
                    field: 1,
                    value: i32_lit(3)
                },
            ]
        }
    );
}

#[test]
fn casts() {
    assert_eq!(
        ret("return (p as i32);"),
        Expr::Cast {
            to: Type::I32,
            operand: b(p())
        }
    );
}

#[test]
fn statements() {
    let r = registry();
    let src = snippet(
        "",
        "let x: f64 = 0.0;
         if (p < 1.0) { x = 1.0; } else { x = 2.0; }
         while (x > 0.0) { x = (x - 1.0); break; }
         repeat 3 { x = (x + 1.0); }
         return x;",
    );
    let l = ok(&load(&src), &r);
    let main = &l.program.functions[fn_id(&l, "top").0];
    let x = LocalId(1);
    let assign = |v: Expr| Stmt::Assign {
        target: x,
        value: v,
    };
    assert_eq!(
        main.body,
        vec![
            assign(f64_lit(0.0)),
            Stmt::If {
                cond: Expr::Binary {
                    op: BinOp::Less,
                    left: b(p()),
                    right: b(f64_lit(1.0))
                },
                then_branch: vec![assign(f64_lit(1.0))],
                else_branch: vec![assign(f64_lit(2.0))],
            },
            Stmt::While {
                cond: Expr::Binary {
                    op: BinOp::Greater,
                    left: b(Expr::Local(x)),
                    right: b(f64_lit(0.0))
                },
                body: vec![
                    assign(Expr::Binary {
                        op: BinOp::Subtract,
                        left: b(Expr::Local(x)),
                        right: b(f64_lit(1.0))
                    }),
                    Stmt::Break
                ],
            },
            Stmt::Repeat {
                count: 3,
                body: vec![assign(Expr::Binary {
                    op: BinOp::Add,
                    left: b(Expr::Local(x)),
                    right: b(f64_lit(1.0))
                })],
            },
            Stmt::Return(Expr::Local(x)),
        ]
    );
    assert_eq!(main.params, 1);
    assert_eq!(main.locals, vec![Type::F64, Type::F64]);
    assert_eq!(main.hosts, vec![HostId(0)]);
}

#[test]
fn const_exprs() {
    let r = registry();
    let src = snippet(
        "const k = (const(c) * 2.0);
         const v = vec3(1.0, 2.0, 3.0).y;
         const n = -(1 as i64);",
        "return const(k);",
    );
    let l = ok(&load(&src), &r);
    let id = |name: &str| {
        l.map
            .consts
            .iter()
            .find(|(_, _, n)| *n == name)
            .unwrap()
            .0
    };
    assert_eq!(
        l.program.consts[id("k").0].init,
        Expr::Binary {
            op: BinOp::Multiply,
            left: b(Expr::Const(id("c"))),
            right: b(f64_lit(2.0))
        }
    );
    assert_eq!(
        l.program.consts[id("v").0].init,
        Expr::Field {
            base: b(Expr::MakeVec3 {
                x: b(f64_lit(1.0)),
                y: b(f64_lit(2.0)),
                z: b(f64_lit(3.0))
            }),
            sel: FieldSel::Vec3(Axis::Y),
            ty: Type::F64
        }
    );
    assert_eq!(
        l.program.consts[id("n").0].init,
        Expr::Unary {
            op: UnOp::Negate,
            operand: b(Expr::Cast {
                to: Type::I64,
                operand: b(i32_lit(1))
            })
        }
    );
}

#[test]
fn items() {
    let r = registry();
    let l = ok(&load(&snippet("", "return p;")), &r);
    let pr = &l.program;
    assert_eq!(pr.hosts, vec![HostDecl { ty: Type::F64 }]);
    assert_eq!(
        pr.tables,
        vec![
            TableDecl {
                kind: TableKind::Perm
            },
            TableDecl {
                kind: TableKind::Sampler
            }
        ]
    );
    let leaf_u = l.map.units.iter().find(|(_, _, n)| *n == "leaf").unwrap().0;
    let u = l.map.units.iter().find(|(_, _, n)| *n == "top_main").unwrap().0;
    assert_eq!(
        pr.units[u.0],
        Unit {
            main: fn_id(&l, "top_main"),
            inputs: vec![InputId(0)],
            tables: vec![TableId(0)],
            hosts: vec![HostId(0)],
            helpers: vec![fn_id(&l, "top")],
            consts: vec![ConstId(0)],
        }
    );
    let one = F64x3 {
        x: 1.0f64.to_bits(),
        y: 1.0f64.to_bits(),
        z: 1.0f64.to_bits(),
    };
    assert_eq!(
        pr.inputs,
        vec![InputDecl {
            source: leaf_u,
            dims: Dims { x: 1, y: 1, z: 1 },
            origin_scale: one.clone(),
            position_scale: one.clone(),
        }]
    );
    assert_eq!(
        pr.entries,
        vec![Entry {
            unit: u,
            dims: Dims { x: 1, y: 1, z: 1 },
            origin_scale: one.clone(),
            position_scale: one,
        }]
    );
    assert_eq!(pr.externs.len(), 2);
    assert_eq!(
        pr.records,
        vec![RecordDecl {
            fields: vec![Type::F64, Type::I32]
        }]
    );
}

#[test]
fn empty_program() {
    let empty_registry = RegistryBuilder::new().build().unwrap();
    let l = ok(&empty(), &empty_registry);
    assert_eq!(
        l.program,
        Program {
            units: vec![],
            functions: vec![],
            consts: vec![],
            hosts: vec![],
            tables: vec![],
            inputs: vec![],
            entries: vec![],
            externs: vec![],
            records: vec![],
        }
    );
}
