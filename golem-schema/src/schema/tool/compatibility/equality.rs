// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::*;

pub(super) fn strictly_equal(expected: &Tool, inner: &Tool) -> bool {
    canonical_tool(expected) == canonical_tool(inner)
}

fn canonical_tool(tool: &Tool) -> Option<serde_json::Value> {
    fn discover(
        ty: &SchemaType,
        defs: &HashMap<TypeId, SchemaTypeDef>,
        ids: &mut HashMap<TypeId, TypeId>,
        order: &mut Vec<TypeId>,
    ) {
        walk_refs(ty, &mut |id| {
            if !ids.contains_key(id) {
                let canonical = TypeId::new(format!("$def{}", ids.len()));
                ids.insert(id.clone(), canonical);
                order.push(id.clone());
                if let Some(def) = defs.get(id) {
                    discover(&def.body, defs, ids, order);
                }
            }
        });
    }

    let mut result = tool.clone();
    erase_ignored_tool_documentation(&mut result);
    let defs: HashMap<_, _> = result
        .schema
        .defs
        .iter()
        .map(|def| (def.id.clone(), def.clone()))
        .collect();
    let mut ids = HashMap::new();
    let mut order = Vec::new();
    for root in tool_schema_roots(&result) {
        discover(root, &defs, &mut ids, &mut order);
    }
    discover(&result.schema.root, &defs, &mut ids, &mut order);

    // Tool validation permits unused definitions, and strict equality includes
    // them. Pick disconnected graph components by their smallest rooted
    // serialization, rather than by their allocation IDs or input order.
    while ids.len() < defs.len() {
        let mut candidates = Vec::new();
        for id in defs.keys().filter(|id| !ids.contains_key(*id)) {
            let mut local_ids = ids.clone();
            let mut local_order = order.clone();
            discover(
                &SchemaType::ref_to(id.clone()),
                &defs,
                &mut local_ids,
                &mut local_order,
            );
            let mut bodies = Vec::new();
            for original in &local_order[order.len()..] {
                let mut def = defs.get(original)?.clone();
                def.id = local_ids.get(original)?.clone();
                rewrite_refs(&mut def.body, &local_ids);
                bodies.push(def);
            }
            candidates.push((serde_json::to_string(&bodies).ok()?, local_ids, local_order));
        }
        let (_, next_ids, next_order) = candidates.into_iter().min_by(|a, b| a.0.cmp(&b.0))?;
        ids = next_ids;
        order = next_order;
    }

    for root in tool_schema_roots_mut(&mut result) {
        rewrite_refs(root, &ids);
    }
    rewrite_refs(&mut result.schema.root, &ids);
    result.schema.defs = order
        .into_iter()
        .map(|original| {
            let mut def = defs.get(&original)?.clone();
            def.id = ids.get(&original)?.clone();
            rewrite_refs(&mut def.body, &ids);
            Some(def)
        })
        .collect::<Option<Vec<_>>>()?;
    serde_json::to_value(result).ok()
}

fn erase_ignored_tool_documentation(tool: &mut Tool) {
    tool.version.clear();
    for node in &mut tool.commands.nodes {
        node.doc = Default::default();
        for option in &mut node.globals.options {
            option.doc = Default::default();
        }
        for flag in &mut node.globals.flags {
            flag.doc = Default::default();
        }
        if let Some(body) = &mut node.body {
            for positional in &mut body.positionals.fixed {
                positional.doc = Default::default();
            }
            if let Some(tail) = &mut body.positionals.tail {
                tail.doc = Default::default();
            }
            for option in &mut body.options {
                option.doc = Default::default();
            }
            for flag in &mut body.flags {
                flag.doc = Default::default();
            }
            if let Some(stdin) = &mut body.stdin {
                stdin.doc = Default::default();
            }
            if let Some(stdout) = &mut body.stdout {
                stdout.doc = Default::default();
            }
            if let Some(stderr) = &mut body.stderr {
                stderr.doc = Default::default();
            }
            if let Some(result) = &mut body.result {
                result.doc = Default::default();
                for formatter in &mut result.formatters {
                    formatter.doc = Default::default();
                }
            }
            for error in &mut body.errors {
                error.doc = Default::default();
            }
        }
    }
    for root in tool_schema_roots_mut(tool) {
        erase_schema_documentation(root);
    }
    erase_schema_documentation(&mut tool.schema.root);
    for def in &mut tool.schema.defs {
        erase_schema_documentation(&mut def.body);
    }
}

fn erase_schema_documentation(ty: &mut SchemaType) {
    ty.metadata_mut().doc = None;
    match ty {
        SchemaType::Record { fields, .. } => fields.iter_mut().for_each(|field| {
            field.metadata.doc = None;
            erase_schema_documentation(&mut field.body);
        }),
        SchemaType::Variant { cases, .. } => cases.iter_mut().for_each(|case| {
            case.metadata.doc = None;
            if let Some(payload) = &mut case.payload {
                erase_schema_documentation(payload);
            }
        }),
        SchemaType::Tuple { elements, .. } => {
            elements.iter_mut().for_each(erase_schema_documentation)
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            erase_schema_documentation(element)
        }
        SchemaType::Map { key, value, .. } => {
            erase_schema_documentation(key);
            erase_schema_documentation(value);
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: crate::schema::schema_type::SecretSpec { inner, .. },
            ..
        } => erase_schema_documentation(inner),
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = &mut spec.ok {
                erase_schema_documentation(ok);
            }
            if let Some(err) = &mut spec.err {
                erase_schema_documentation(err);
            }
        }
        SchemaType::Union { spec, .. } => spec.branches.iter_mut().for_each(|branch| {
            branch.metadata.doc = None;
            erase_schema_documentation(&mut branch.body);
        }),
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(inner) = inner {
                erase_schema_documentation(inner);
            }
        }
        _ => {}
    }
}

fn walk_refs(ty: &SchemaType, visit: &mut impl FnMut(&TypeId)) {
    match ty {
        SchemaType::Ref { id, .. } => visit(id),
        SchemaType::Record { fields, .. } => fields.iter().for_each(|f| walk_refs(&f.body, visit)),
        SchemaType::Variant { cases, .. } => cases
            .iter()
            .filter_map(|c| c.payload.as_ref())
            .for_each(|t| walk_refs(t, visit)),
        SchemaType::Tuple { elements, .. } => elements.iter().for_each(|t| walk_refs(t, visit)),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            walk_refs(element, visit)
        }
        SchemaType::Map { key, value, .. } => {
            walk_refs(key, visit);
            walk_refs(value, visit);
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: crate::schema::schema_type::SecretSpec { inner, .. },
            ..
        } => walk_refs(inner, visit),
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &spec.ok {
                walk_refs(t, visit)
            }
            if let Some(t) = &spec.err {
                walk_refs(t, visit)
            }
        }
        SchemaType::Union { spec, .. } => {
            spec.branches.iter().for_each(|b| walk_refs(&b.body, visit))
        }
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                walk_refs(t, visit)
            }
        }
        _ => {}
    }
}

fn rewrite_refs(ty: &mut SchemaType, ids: &HashMap<TypeId, TypeId>) {
    match ty {
        SchemaType::Ref { id, .. } => {
            if let Some(new) = ids.get(id) {
                *id = new.clone();
            }
        }
        SchemaType::Record { fields, .. } => fields
            .iter_mut()
            .for_each(|f| rewrite_refs(&mut f.body, ids)),
        SchemaType::Variant { cases, .. } => cases
            .iter_mut()
            .filter_map(|c| c.payload.as_mut())
            .for_each(|t| rewrite_refs(t, ids)),
        SchemaType::Tuple { elements, .. } => {
            elements.iter_mut().for_each(|t| rewrite_refs(t, ids))
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            rewrite_refs(element, ids)
        }
        SchemaType::Map { key, value, .. } => {
            rewrite_refs(key, ids);
            rewrite_refs(value, ids);
        }
        SchemaType::Option { inner, .. }
        | SchemaType::Secret {
            spec: crate::schema::schema_type::SecretSpec { inner, .. },
            ..
        } => rewrite_refs(inner, ids),
        SchemaType::Result { spec, .. } => {
            if let Some(t) = &mut spec.ok {
                rewrite_refs(t, ids)
            }
            if let Some(t) = &mut spec.err {
                rewrite_refs(t, ids)
            }
        }
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter_mut()
            .for_each(|b| rewrite_refs(&mut b.body, ids)),
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            if let Some(t) = inner {
                rewrite_refs(t, ids)
            }
        }
        _ => {}
    }
}

fn tool_schema_roots(tool: &Tool) -> Vec<&SchemaType> {
    let mut roots = Vec::new();
    for node in &tool.commands.nodes {
        collect_command_roots(&node.globals, node.body.as_ref(), &mut roots);
    }
    roots
}

fn tool_schema_roots_mut(tool: &mut Tool) -> Vec<&mut SchemaType> {
    let mut roots = Vec::new();
    for node in &mut tool.commands.nodes {
        collect_command_roots_mut(&mut node.globals, node.body.as_mut(), &mut roots);
    }
    roots
}

fn collect_command_roots<'a>(
    globals: &'a super::super::Globals,
    body: Option<&'a CommandBody>,
    roots: &mut Vec<&'a SchemaType>,
) {
    for option in &globals.options {
        roots.push(option_type(&option.shape));
    }
    if let Some(body) = body {
        roots.extend(body.positionals.fixed.iter().map(|p| &p.type_));
        if let Some(tail) = &body.positionals.tail {
            roots.push(&tail.item_type);
        }
        roots.extend(body.options.iter().map(|o| option_type(&o.shape)));
        if let Some(result) = &body.result {
            roots.push(&result.type_);
        }
        roots.extend(body.errors.iter().filter_map(|e| e.payload.as_ref()));
    }
}

fn collect_command_roots_mut<'a>(
    globals: &'a mut super::super::Globals,
    body: Option<&'a mut CommandBody>,
    roots: &mut Vec<&'a mut SchemaType>,
) {
    for option in &mut globals.options {
        roots.push(option_type_mut(&mut option.shape));
    }
    if let Some(body) = body {
        roots.extend(body.positionals.fixed.iter_mut().map(|p| &mut p.type_));
        if let Some(tail) = &mut body.positionals.tail {
            roots.push(&mut tail.item_type);
        }
        roots.extend(
            body.options
                .iter_mut()
                .map(|o| option_type_mut(&mut o.shape)),
        );
        if let Some(result) = &mut body.result {
            roots.push(&mut result.type_);
        }
        roots.extend(body.errors.iter_mut().filter_map(|e| e.payload.as_mut()));
    }
}

fn option_type(shape: &OptionShape) -> &SchemaType {
    match shape {
        OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => ty,
        OptionShape::RepeatableList(shape) => &shape.item_type,
        OptionShape::RepeatableMap(shape) => &shape.map_type,
    }
}

fn option_type_mut(shape: &mut OptionShape) -> &mut SchemaType {
    match shape {
        OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => ty,
        OptionShape::RepeatableList(shape) => &mut shape.item_type,
        OptionShape::RepeatableMap(shape) => &mut shape.map_type,
    }
}
