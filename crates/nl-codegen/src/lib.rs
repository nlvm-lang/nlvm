mod class_table;
mod closure;
pub mod error;
mod expr;
mod native_generics;
mod stdlib;
mod stmt;
mod type_desc;

use std::collections::HashMap;

use nl_bytecode::{
    class_flags, field_flags, method_flags, ConstantPool, HashAlgo, MethodDescriptor, Module,
};
use nl_syntax::ast::{ClassDecl, MethodKind, SourceFile, SourceItem, Type, Visibility};

use class_table::{build_class_table, fqcn_of, import_map, resolve_type, ClassInfo};
pub use error::CodegenError;
use expr::{expr_ty_of, Emitter, MethodSig};
use type_desc::method_descriptor;

/// Compiles a whole program (every file that will be linked together) in one
/// pass: a shared class table is built first so `new`/field access/instance
/// method calls that cross file boundaries resolve to real constant-pool
/// entries. See `nl_vm::Program` for how these modules are linked at load
/// time.
pub fn compile_program(files: &[SourceFile]) -> Result<Vec<Module>, CodegenError> {
    // Built-in exception classes (nl_syntax::prelude) are implicitly part of
    // every program — see class_table::import_map, which seeds their simple
    // names so user code can reference them without a `use`. Prepended
    // *before* expansion (not after): the prelude's `Box<T>` (vm.md § Ref
    // parameters (boxing)) is itself a template, and `nl_syntax::monomorphize
    // ::expand` only ever monomorphizes templates it can see in its own
    // input. nl-sema expands the exact same combined input the same way
    // (see its `check_compile`), so both crates always agree on the
    // expanded program.
    let mut unexpanded = nl_syntax::prelude::files();
    unexpanded.extend(files.to_vec());

    // Template classes (specs.md § Template class) are expanded into
    // ordinary monomorphized classes before anything else sees them — see
    // nl_syntax::monomorphize.
    let all_files = nl_syntax::monomorphize::expand(unexpanded);

    let classes = build_class_table(&all_files);
    let mut modules = Vec::new();
    for file in &all_files {
        modules.extend(compile_file(file, &all_files, &classes)?);
    }
    Ok(modules)
}

/// Single-file convenience wrapper — still valid for programs that don't
/// reference any other class (e.g. the milestone 1-4 fixtures). `compile_program`
/// always also returns the built-in prelude's modules, so the caller's own
/// module is found by name rather than assumed to be at a fixed index.
pub fn compile_source_file(file: &SourceFile) -> Result<Module, CodegenError> {
    let fqcn = fqcn_of(file);
    let modules = compile_program(std::slice::from_ref(file))?;
    Ok(modules
        .into_iter()
        .find(|m| m.this_class_name() == Some(fqcn.as_str()))
        .expect("compile_program always compiles the input file's own module"))
}

/// Returns the file's own module first, followed by any synthetic closure
/// classes generated while compiling its methods (vm.md § Closures — "the
/// compiler generates a synthetic class for each closure").
fn compile_file(
    file: &SourceFile,
    all_files: &[SourceFile],
    classes: &HashMap<String, ClassInfo>,
) -> Result<Vec<Module>, CodegenError> {
    let imports = import_map(file, all_files);
    let fqcn = fqcn_of(file);
    let mut cp = ConstantPool::new();
    let this_class = cp.add_class(&fqcn);

    match &file.item {
        SourceItem::Interface(_) => Ok(vec![Module {
            version: nl_bytecode::module::VERSION,
            constant_pool: cp,
            this_class,
            class_flags: class_flags::INTERFACE,
            super_class: 0,
            interfaces: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            hash_algo: HashAlgo::Sha256,
        }]),
        SourceItem::Class(class) => {
            let super_class = match &class.extends {
                Some(name) => {
                    let super_fqcn = imports.get(name).cloned().unwrap_or_else(|| name.clone());
                    cp.add_class(&super_fqcn)
                }
                None => 0,
            };
            let interfaces = class
                .implements
                .iter()
                .map(|name| {
                    let iface_fqcn = imports.get(name).cloned().unwrap_or_else(|| name.clone());
                    cp.add_class(&iface_fqcn)
                })
                .collect();

            let fields = class
                .fields
                .iter()
                .map(|f| {
                    let name_index = cp.add_utf8(f.name.clone());
                    let resolved_ty = resolve_type(&f.ty, &imports);
                    let type_index = cp.add_type_desc(&type_desc::type_descriptor(&resolved_ty));
                    let mut flags = visibility_field_flag(f.visibility);
                    if f.is_static {
                        flags |= field_flags::STATIC;
                    }
                    if f.readonly {
                        flags |= field_flags::READONLY;
                    }
                    nl_bytecode::FieldDescriptor {
                        flags,
                        name_index,
                        type_index,
                    }
                })
                .collect();

            // First pass: register every static method's signature so bare
            // (unqualified) calls resolve regardless of declaration order —
            // instance methods/constructors are only reachable via `expr.m(...)`
            // /`new`/`this(...)`, resolved directly at their call site instead.
            let mut static_sigs = HashMap::new();
            for m in &class.methods {
                if m.is_static && m.kind == MethodKind::Normal {
                    let name_index = cp.add_utf8(m.name.clone());
                    let params: Vec<Type> = m
                        .params
                        .iter()
                        .map(|p| resolve_type(&p.ty, &imports))
                        .collect();
                    let is_ref: Vec<bool> = m.params.iter().map(|p| p.is_ref).collect();
                    let return_ty = resolve_type(&m.return_type, &imports);
                    // vm.md § Ref parameters (boxing) — a `ref` parameter's
                    // *physical* type in the descriptor is `Box<T>`, not `T`.
                    let cc_params = class_table::calling_convention_params(&params, &is_ref);
                    let descriptor = method_descriptor(&cc_params, &return_ty);
                    let descriptor_index = cp.add_type_desc(&descriptor);
                    let method_ref_index =
                        cp.add_method_ref(this_class, name_index, descriptor_index);
                    static_sigs.insert(
                        m.name.clone(),
                        MethodSig {
                            param_types: params.iter().map(expr_ty_of).collect(),
                            param_names: m.params.iter().map(|p| p.name.clone()).collect(),
                            defaults: m.params.iter().map(|p| p.default.clone()).collect(),
                            is_ref,
                            return_ty: expr_ty_of(&return_ty),
                            method_ref_index,
                        },
                    );
                }
            }

            let mut methods = Vec::with_capacity(class.methods.len());
            let mut closure_modules = Vec::new();
            // specs.md § Abstract classes and methods — an abstract method
            // has no body and is never itself instantiable/directly callable
            // (E032 rejects `new` on its class; E033 guarantees every
            // concrete subclass provides a real override, which virtual
            // dispatch always resolves to first) — nothing to emit.
            for (method_index, m) in class.methods.iter().filter(|m| !m.is_abstract).enumerate() {
                let (descriptor, closures) = compile_method(
                    m.name.as_str(),
                    method_index,
                    m,
                    class,
                    &mut cp,
                    this_class,
                    &fqcn,
                    &imports,
                    classes,
                    &static_sigs,
                )?;
                methods.push(descriptor);
                closure_modules.extend(closures);
            }

            let mut flags = 0u16;
            if class.is_readonly {
                flags |= class_flags::READONLY;
            }
            if class.is_enum {
                flags |= class_flags::ENUM;
            }
            let mut modules = vec![Module {
                version: nl_bytecode::module::VERSION,
                constant_pool: cp,
                this_class,
                class_flags: flags,
                super_class,
                interfaces,
                fields,
                methods,
                hash_algo: HashAlgo::Sha256,
            }];
            modules.extend(closure_modules);
            Ok(modules)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_method(
    name: &str,
    method_index: usize,
    method: &nl_syntax::ast::MethodDecl,
    class: &ClassDecl,
    cp: &mut ConstantPool,
    this_class: u16,
    this_fqcn: &str,
    imports: &HashMap<String, String>,
    classes: &HashMap<String, ClassInfo>,
    static_sigs: &HashMap<String, MethodSig>,
) -> Result<(MethodDescriptor, Vec<Module>), CodegenError> {
    let _ = class;
    let name_index = cp.add_utf8(name.to_string());
    let resolved_params: Vec<Type> = method
        .params
        .iter()
        .map(|p| resolve_type(&p.ty, imports))
        .collect();
    let is_ref: Vec<bool> = method.params.iter().map(|p| p.is_ref).collect();
    let resolved_return = resolve_type(&method.return_type, imports);
    // vm.md § Ref parameters (boxing) — a `ref` parameter's *physical* type
    // in this method's own descriptor is `Box<T>`, not `T` (must match
    // what every call site builds its `method_ref`/`INVOKE_*` against).
    let cc_params = class_table::calling_convention_params(&resolved_params, &is_ref);
    let descriptor = method_descriptor(&cc_params, &resolved_return);
    let descriptor_index = cp.add_type_desc(&descriptor);

    let mut emitter = Emitter::new(
        cp,
        static_sigs,
        classes,
        imports,
        this_class,
        this_fqcn.to_string(),
    );
    emitter.closure_name_prefix = format!("{this_fqcn}$m{method_index}");
    emitter.push_scope();
    if !method.is_static {
        emitter.declare_local(
            "this".to_string(),
            expr::ExprTy::Object(this_fqcn.to_string()),
        );
    }
    for ((param, resolved_ty), r) in method.params.iter().zip(&resolved_params).zip(&is_ref) {
        if *r {
            emitter.declare_ref_param(param.name.clone(), expr_ty_of(resolved_ty));
        } else {
            emitter.declare_local(param.name.clone(), expr_ty_of(resolved_ty));
        }
    }
    for stmt in &method.body {
        emitter.compile_stmt(stmt)?;
    }
    if resolved_return == Type::Void {
        emitter.code.push(nl_bytecode::Opcode::Return as u8);
    }
    emitter.pop_scope();

    // Metadata only at this layer — checked-exception propagation (E015)
    // and override compatibility (E016/E017) are enforced by nl-sema
    // (crate::checker), not re-derived from this bytecode-level list.
    let throws_types: Vec<u16> = method
        .throws
        .iter()
        .map(|name| {
            let fqcn = imports.get(name).cloned().unwrap_or_else(|| name.clone());
            emitter.cp.add_class(&fqcn)
        })
        .collect();

    let mut flags = visibility_method_flag(method.visibility);
    if method.is_static {
        flags |= method_flags::STATIC;
    }
    match method.kind {
        MethodKind::Constructor => flags |= method_flags::CONSTRUCTOR,
        MethodKind::Destructor => flags |= method_flags::DESTRUCTOR,
        MethodKind::Normal => {}
    }

    let descriptor = MethodDescriptor {
        flags,
        name_index,
        descriptor_index,
        throws_types,
        max_locals: emitter.max_locals(),
        max_stack: emitter.max_stack(),
        code: emitter.code,
        exception_table: emitter.exception_table,
        line_table: emitter.line_table,
    };
    Ok((descriptor, emitter.closures))
}

fn visibility_field_flag(v: Visibility) -> u16 {
    match v {
        Visibility::Public => field_flags::PUBLIC,
        Visibility::Protected => field_flags::PROTECTED,
        Visibility::Private => field_flags::PRIVATE,
    }
}

fn visibility_method_flag(v: Visibility) -> u16 {
    match v {
        Visibility::Public => method_flags::PUBLIC,
        Visibility::Protected => method_flags::PROTECTED,
        Visibility::Private => method_flags::PRIVATE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// vm.md § Method descriptor (line-number table): entries are sorted by
    /// ascending `start_pc`, one per source line change, so a `main` whose
    /// statements sit on known lines should produce a line table whose
    /// `line`s match those exactly (no gaps, no drift from the coalescing in
    /// `Emitter::record_line`).
    #[test]
    fn line_table_tracks_source_lines() {
        let src = "namespace test;\n\
                    class Program {\n\
                    \x20   public static int main(string[] args) {\n\
                    \x20       int x = 1;\n\
                    \x20       int y = 2;\n\
                    \x20       if (x < y) {\n\
                    \x20           x = y;\n\
                    \x20       }\n\
                    \x20       return x;\n\
                    \x20   }\n\
                    }\n";
        let file = nl_syntax::parse_source_file(src, "Program.nl".to_string()).unwrap();
        let module = compile_source_file(&file).unwrap();
        let method = module.find_method("main").unwrap();

        assert!(
            !method.line_table.is_empty(),
            "expected a non-empty line table for a method with real statements"
        );

        // start_pc strictly increasing (one entry per statement boundary,
        // deduped by line — see `record_line`) and within the method's code.
        let mut prev_pc = None;
        for entry in &method.line_table {
            if let Some(p) = prev_pc {
                assert!(
                    entry.start_pc > p,
                    "line table entries must have strictly increasing start_pc"
                );
            }
            assert!((entry.start_pc as usize) < method.code.len());
            prev_pc = Some(entry.start_pc);
        }

        let lines: Vec<u32> = method.line_table.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![4, 5, 6, 7, 9]);
    }
}
