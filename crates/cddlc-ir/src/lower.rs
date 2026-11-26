/// AST → IR lowering.
///
/// Walks the parser [`CddlModule`] and produces a resolved [`IrModule`].
///
/// Stages (in order):
/// 1. Collect all rule names into a resolution table (forward-decl pass)
/// 2. Collect generic rule templates
/// 3. Lower each non-generic rule:
///    a. Resolve type references (prelude, user rules, generics)
///    b. Lower control operators → Constraints
///    c. Propagate pragma metadata (capacity, doc, deprecated, regex-hook)
///    d. Classify each rule as Struct / Enum / Array / Map / Alias
/// 4. Topological sort
/// 5. Emit LowerResult

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use smol_str::SmolStr;

use cddlc_parser::{
    CddlModule, GroupChoice, MemberKind, Occurrence as AstOcc,
    Pragma, PragmaValue, Rule, RuleKind, Spanned, Type, Type1, Type2,
    Value,
};

use crate::error::{IrError, IrWarning, LowerResult};
use crate::lower_constraints::{lower_type_op, lower_value};
use crate::prelude::is_prelude;
use crate::types::{
    AliasDef, ArrayDef, Capacity, CborTag, Constraint, EnumDef, EnumVariant,
    FieldDef, FieldKey, IrModule, LiteralValue, MapDef, MapRest, Occurrence,
    Primitive, Sources, StructDef, StructEncoding, TextEncoding, TypeDef, TypeRef,
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower a parsed [`CddlModule`] into a resolved [`IrModule`].
///
/// `default_capacity` and `default_str_capacity` come from CLI flags;
/// they are used when a rule has no `@capacity`/`@str-capacity` pragma.
pub fn lower(
    module:               &CddlModule,
    default_capacity:     usize,
    default_str_capacity: usize,
) -> Result<LowerResult, IrError> {
    let mut ctx = LowerCtx::new(module, default_capacity, default_str_capacity);
    ctx.run()
}

// ── Lowering context ──────────────────────────────────────────────────────────

struct LowerCtx<'m> {
    module:                &'m CddlModule,
    default_capacity:      usize,
    _default_str_capacity: usize,

    /// All rule names defined in this module (for forward-ref resolution).
    known_rules: HashSet<SmolStr>,

    /// Generic rule templates: name → (params, rule).
    generics: HashMap<SmolStr, (Vec<SmolStr>, &'m Rule)>,

    /// Output accumulator — in definition order (topo sort applied later).
    output: IndexMap<SmolStr, TypeDef>,

    /// Where each type and field was written (dgtlrift/cddlc#9).
    sources: Sources,

    warnings: Vec<IrWarning>,

    /// The type an inline map or array of just a group rule (`{ g }`, `[ g ]`)
    /// was lowered to, keyed by (is it an array, the group's name): one type
    /// for every use, which also ends a group that recurses through one
    /// (`schema = ( elements: { schema } // ... )`, RFC 8927).
    inline_groups: HashMap<(bool, SmolStr), SmolStr>,
}

impl<'m> LowerCtx<'m> {
    fn new(
        module:               &'m CddlModule,
        default_capacity:     usize,
        default_str_capacity: usize,
    ) -> Self {
        Self {
            module,
            default_capacity,
            _default_str_capacity: default_str_capacity,
            known_rules:  HashSet::new(),
            generics:     HashMap::new(),
            output:       IndexMap::new(),
            sources:      Sources::default(),
            warnings:     Vec::new(),
            inline_groups: HashMap::new(),
        }
    }

    fn run(&mut self) -> Result<LowerResult, IrError> {
        // Pass 1: collect all defined names
        for sr in &self.module.rules {
            self.known_rules.insert(sr.node.name.node.0.clone());
        }

        // Pass 2: collect generic templates
        for sr in &self.module.rules {
            let rule = &sr.node;
            if !rule.generic_params.is_empty() {
                let params: Vec<SmolStr> = rule.generic_params
                    .iter()
                    .map(|p| p.node.0.clone())
                    .collect();
                self.generics.insert(rule.name.node.0.clone(), (params, rule));
            }
        }

        // Pass 3: lower non-generic rules
        for sr in &self.module.rules {
            let rule = &sr.node;
            if rule.generic_params.is_empty() {
                self.lower_rule(rule)?;
            }
        }

        // The keys of a map's other entries are text, integers, or a choice of
        // them (COSE's `* label => values` with `label = int / tstr`,
        // dgtlrift/cddlc#136); checked once every alias is lowered.
        for def in self.output.values() {
            if let TypeDef::Struct(st) = def {
                if let Some(rest) = &st.rest {
                    if !self.is_scalar_key(&rest.key_ty, 0) {
                        return Err(IrError::Unsupported {
                            rule:   st.name.to_string(),
                            reason: "a computed key other than text or integers alongside named members".into(),
                        });
                    }
                }
            }
        }
        // A key that's an alias of `tstr` is `tstr`: backends hold text keys
        // as text whatever the key's name.
        let text_keyed: Vec<SmolStr> = self.output.iter()
            .filter_map(|(name, def)| match def {
                TypeDef::Struct(st) => st.rest.as_ref()
                    .filter(|r| matches!(r.key_ty, TypeRef::Named(_)) && self.is_tstr(&r.key_ty, 0))
                    .map(|_| name.clone()),
                _ => None,
            })
            .collect();
        for name in text_keyed {
            if let Some(TypeDef::Struct(st)) = self.output.get_mut(&name) {
                if let Some(r) = st.rest.as_mut() {
                    r.key_ty = TypeRef::Primitive(Primitive::Tstr);
                }
            }
        }

        // A group's entries are inlined where it is used (dgtlrift/cddlc#83);
        // groups aren't types, so one nothing refers to as a type is dropped
        // (`$$g` isn't an identifier in any target language).
        let root_name = self.module.rules.first().map(|sr| sr.node.name.node.0.clone());
        let referenced: HashSet<SmolStr> = self.output.values().flat_map(collect_deps).collect();
        let unused_groups: Vec<SmolStr> = self.output.keys()
            .filter(|n| Some(*n) != root_name.as_ref() && !referenced.contains(*n))
            .filter(|n| self.group_alternatives(n).is_some())
            .cloned()
            .collect();
        // So are the types made for one (`<group>-alt1`, `<group>-<member>`)
        // that nothing else refers to.
        let rule_names: HashSet<SmolStr> = self.module.rules.iter().map(|sr| sr.node.name.node.0.clone()).collect();
        let mut removed = unused_groups;
        let mut i = 0;
        while i < removed.len() {
            let n = removed[i].clone();
            self.output.shift_remove(&n);
            self.sources.types.shift_remove(&n);
            let referenced: HashSet<SmolStr> = self.output.values().flat_map(collect_deps).collect();
            let prefix = format!("{n}-");
            let orphans: Vec<SmolStr> = self.output.keys()
                .filter(|k| k.starts_with(&prefix) && !rule_names.contains(*k) && !referenced.contains(*k))
                .cloned()
                .collect();
            removed.extend(orphans);
            i += 1;
        }

        // Variant names unique within each enum (`"Infinity"` / `"-Infinity"`
        // are `Infinity` / `MinusInfinity`, but `"a-b"` / `"a_b"` are both `AB`).
        for def in self.output.values_mut() {
            if let TypeDef::Enum(e) = def {
                let mut seen: HashSet<SmolStr> = HashSet::new();
                for (i, v) in e.variants.iter_mut().enumerate() {
                    if !seen.insert(v.name.clone()) {
                        let mut name: SmolStr = format!("{}{i}", v.name).into();
                        while seen.contains(&name) {
                            name = format!("{name}_").into();
                        }
                        seen.insert(name.clone());
                        v.name = name;
                    }
                }
            }
        }

        // Pass 4: topological sort
        let sorted = topo_sort(&self.output)?;

        let root: SmolStr = self.module.rules
            .first()
            .map(|sr| sr.node.name.node.0.clone())
            .unwrap_or_default();

        let mut ir = IrModule::new(
            self.module.source_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "module".into()),
            root,
        );
        for name in sorted {
            if let Some(def) = self.output.swap_remove(&name) {
                ir.insert(name, def);
            }
        }
        ir.sources = std::mem::take(&mut self.sources);

        Ok(LowerResult {
            module:   ir,
            warnings: std::mem::take(&mut self.warnings),
        })
    }

    // ── Rule dispatch ─────────────────────────────────────────────────────────

    fn lower_rule(&mut self, rule: &'m Rule) -> Result<(), IrError> {
        let name: SmolStr = rule.name.node.0.clone();

        // Extract pragma metadata
        let mut meta = PragmaMeta::from_pragmas(&rule.pragmas);
        // An `@doc` pragma wins; otherwise the comments written above the rule
        // (dgtlrift/cddlc#8).
        if meta.doc.is_none() {
            meta.doc = rule.doc.clone();
        }
        if let Some(s) = &rule.source {
            self.sources.types.insert(name.clone(), s.into());
        }

        match &rule.kind {
            RuleKind::Type { ty, extend: false } => {
                let def = self.lower_type_rule(&name, &ty.node, &meta)?;
                self.output.insert(name, def);
            }
            RuleKind::Type { ty, extend: true } => {
                // `$$g //= ( a: uint )` adds to a group (dgtlrift/cddlc#83).
                match single_type2(&ty.node) {
                    Some(Type2::Group(g)) => self.lower_group_extend(&name, &g.node.choices)?,
                    _ => self.lower_type_extend(&name, &ty.node)?,
                }
            }
            RuleKind::Group { group, extend: false } => {
                // `t = (int / uint)` is a parenthesized type, not a group.
                let expanded = self.expand_groups(&name, &group.node.choices, false)?;
                let def = match expanded.as_slice() {
                    [gc] if matches!(gc.entries.as_slice(), [e]
                        if e.node.occurrence.is_none() && matches!(e.node.kind, MemberKind::Type(_))) =>
                    {
                        let MemberKind::Type(ty) = &gc.entries[0].node.kind else { unreachable!() };
                        self.lower_type_rule(&name, &ty.node, &meta)?
                    }
                    // A group rule with `(` — treat as struct
                    _ => self.lower_group_as_struct(&name, &group.node.choices, &meta)?,
                };
                self.output.insert(name, def);
            }
            RuleKind::Group { group, extend: true } => {
                self.lower_group_extend(&name, &group.node.choices)?;
            }
        }
        Ok(())
    }

    // ── Type rule lowering ────────────────────────────────────────────────────

    fn lower_type_rule(
        &mut self,
        name: &str,
        ty:   &Type,
        meta: &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        // A type with multiple choices → Enum
        if ty.choices.len() > 1 {
            return self.lower_as_enum(name, ty, meta);
        }

        let t1 = &ty.choices[0].node;

        // Classify the base type2
        match &t1.base.node {
            // Map → Struct
            Type2::Map(group) => {
                self.lower_map_as_struct(name, &group.node.choices, meta)
            }

            // Array → ArrayDef or StructDef (tuple-style)
            Type2::Array(group) => {
                self.lower_array_type(name, &group.node.choices, meta)
            }

            // Tagged type: #6.NN(inner)
            Type2::Tagged { tag, inner } => {
                let cbor_tag = CborTag(*tag);
                let mut inner_meta = meta.clone();
                inner_meta.tag = Some(cbor_tag);
                self.lower_tagged_type(name, &inner.node, &inner_meta)
            }

            // Typename reference — alias or forward ref
            Type2::Typename { name: ref_name, args } => {
                if args.is_empty() {
                    self.lower_alias(name, ref_name.node.as_str(), t1, meta)
                } else {
                    // Generic instantiation
                    self.lower_generic_instance(
                        name,
                        ref_name.node.as_str(),
                        args,
                        meta,
                    )
                }
            }

            // Value literal used as a type → single-variant enum / alias,
            // or (with a chained range/control op) a constrained alias, e.g.
            // `byte = 0..255`.
            Type2::Value(v) => {
                let lit = lower_value(v);
                let mut constraints = Vec::new();
                if t1.ops.is_empty() {
                    constraints.push(Constraint::Eq(lit.clone()));
                } else {
                    for op in &t1.ops {
                        let cs = self.lower_op(op, &t1.base.node, name, meta.regex_hook.as_deref(), 0)?;
                        constraints.extend(cs);
                    }
                }
                let def = AliasDef {
                    name:        name.into(),
                    ty:          literal_to_typeref(&lit),
                    constraints,
                    tagged:      meta.tag,
                    doc:         meta.doc.clone(),
                    deprecated:  meta.deprecated,
                };
                Ok(TypeDef::Alias(def))
            }

            // `#mt.ai` → the type it stands for (RFC 8610 §3.6)
            Type2::MajorType(mt, ai) => {
                let (ty, constraints) = major_type(*mt, *ai);
                Ok(TypeDef::Alias(AliasDef {
                    name:        name.into(),
                    ty,
                    constraints,
                    tagged:      meta.tag,
                    doc:         meta.doc.clone(),
                    deprecated:  meta.deprecated,
                }))
            }

            // Any → alias to Any
            Type2::Any => {
                Ok(TypeDef::Alias(AliasDef {
                    name:        name.into(),
                    ty:          TypeRef::Primitive(Primitive::Any),
                    constraints: vec![],
                    tagged:      meta.tag,
                    doc:         meta.doc.clone(),
                    deprecated:  meta.deprecated,
                }))
            }

            Type2::Unwrap(ident) => {
                Ok(TypeDef::Alias(AliasDef {
                    name:        name.into(),
                    ty:          TypeRef::Named(ident.node.0.clone()),
                    constraints: vec![],
                    tagged:      meta.tag,
                    doc:         meta.doc.clone(),
                    deprecated:  meta.deprecated,
                }))
            }

            // Group / Paren — treat same as map
            Type2::Group(group) => {
                self.lower_map_as_struct(name, &group.node.choices, meta)
            }

            // `&colors`, `&( red: 1, green: 2 )` — RFC 8610 §3.9: the choice of
            // the group's member values (dgtlrift/cddlc#85).
            Type2::EnumTypename { .. } | Type2::EnumGroup(_) => {
                let values = self.enumeration_values(name, &t1.base.node)?;
                self.lower_type_rule(name, &values, meta)
            }
        }
    }

    // ── Enum (type choice) ────────────────────────────────────────────────────

    fn lower_as_enum(
        &mut self,
        name: &str,
        ty:   &Type,
        meta: &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        let mut variants = Vec::new();
        for (i, choice) in ty.choices.iter().enumerate() {
            let t2 = &choice.node.base.node;
            let (var_name, type_ref, constraints) = self.classify_enum_variant(t2, i, name)?;
            variants.push(EnumVariant { name: var_name, ty: type_ref, constraints });
        }
        Ok(TypeDef::Enum(EnumDef {
            name:       name.into(),
            variants,
            tagged:     meta.tag,
            doc:        meta.doc.clone(),
            deprecated: meta.deprecated,
        }))
    }

    fn classify_enum_variant(
        &mut self,
        t2:        &Type2,
        index:     usize,
        rule_name: &str,
    ) -> Result<(SmolStr, TypeRef, Vec<Constraint>), IrError> {
        match t2 {
            Type2::Value(Value::Text(s)) => {
                let var_name = literal_variant_name(s);
                let lit = LiteralValue::Text(s.clone());
                Ok((var_name.into(), TypeRef::Primitive(Primitive::Tstr), vec![Constraint::Eq(lit)]))
            }
            Type2::Value(Value::Uint(n)) => {
                let lit = LiteralValue::Uint(*n);
                Ok((format!("Variant{n}").into(), TypeRef::Primitive(Primitive::Uint), vec![Constraint::Eq(lit)]))
            }
            Type2::Value(Value::Int(n)) => {
                let lit = LiteralValue::Int(*n);
                Ok((format!("Variant{n}").into(), TypeRef::Primitive(Primitive::Int), vec![Constraint::Eq(lit)]))
            }
            Type2::Typename { name, .. } => {
                let n = name.node.as_str();
                let tr = self.resolve_typeref(n, rule_name)?;
                Ok((pascal_case(n).into(), tr, vec![]))
            }
            // An inline array or map (`null / [a, b]`, `{ + text => text } / null`)
            // is a type of its own, `<rule>-alt<i>`, as a member's is
            // (dgtlrift/cddlc#138).
            Type2::Array(group) | Type2::Map(group) => {
                let mut name: SmolStr = format!("{rule_name}-alt{index}").into();
                while self.output.contains_key(&name) || self.known_rules.contains(&name) {
                    name = format!("{name}-type").into();
                }
                self.known_rules.insert(name.clone());
                let meta = PragmaMeta::default();
                let def = if matches!(t2, Type2::Array(_)) {
                    self.lower_array_type(&name, &group.node.choices, &meta)?
                } else {
                    self.lower_map_as_struct(&name, &group.node.choices, &meta)?
                };
                self.output.insert(name.clone(), def);
                Ok((format!("Variant{index}").into(), TypeRef::Named(name), vec![]))
            }
            _ => {
                Ok((format!("Variant{index}").into(),
                    TypeRef::Primitive(Primitive::Any), vec![]))
            }
        }
    }

    // ── Struct (named map) ────────────────────────────────────────────────────

    fn lower_map_as_struct(
        &mut self,
        name:    &str,
        choices: &[GroupChoice],
        meta:    &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        self.lower_struct(name, choices, meta, true)
    }

    /// A struct from a group's members: a map's (`map`), or a record's, whose
    /// members may repeat.
    fn lower_struct(
        &mut self,
        name:    &str,
        choices: &[GroupChoice],
        meta:    &PragmaMeta,
        map:     bool,
    ) -> Result<TypeDef, IrError> {
        let expanded = self.expand_groups(name, choices, false)?;
        if expanded.len() > 1 {
            return self.lower_alternatives(name, &expanded, meta, false);
        }
        let choices = expanded.as_slice();
        // A map whose only member has a computed key (`{ * tstr => uint }`) is a
        // table: any number of entries whose keys and values have the given
        // types (RFC 8610 §3.5.1, dgtlrift/cddlc#82).
        if let [gc] = choices {
            if let [entry] = gc.entries.as_slice() {
                if let MemberKind::KeyValue { key, value, .. } = &entry.node.kind {
                    if let cddlc_parser::MemberKey::Type(key_ty) = &key.node {
                        return self.lower_table(name, entry, key_ty, &value.node, meta);
                    }
                }
            }
        }

        // For now we take the first (and usually only) group choice.
        // Group choices (//= extensions) are handled by lower_group_extend.
        // A member with a computed key holds the map's other entries
        // (`{ a: uint, * tstr => any }`, dgtlrift/cddlc#82).
        let mut fields = Vec::new();
        let mut computed = None;
        for e in choices.iter().flat_map(|gc| gc.entries.iter()) {
            match &e.node.kind {
                MemberKind::KeyValue { key, value, .. }
                    if matches!(key.node, cddlc_parser::MemberKey::Type(_)) =>
                {
                    let cddlc_parser::MemberKey::Type(key_ty) = &key.node else { unreachable!() };
                    if computed.is_some() {
                        return Err(IrError::Unsupported {
                            rule:   name.into(),
                            reason: "a map with more than one member with a computed key".into(),
                        });
                    }
                    computed = Some(self.lower_computed_entry(name, e, key_ty, &value.node, meta)?);
                }
                _ => fields.push(self.lower_member_entry(e, name)?),
            }
        }
        // A map holds a key at most once (RFC 8610 §3.5), so a member that may
        // repeat (`* b: tstr`) is optional, and one that must appear (`+ b`)
        // is required (dgtlrift/cddlc#184).
        for f in fields.iter_mut().filter(|_| map) {
            if !f.occurrence.is_repeated() {
                continue;
            }
            let (min, max) = match &f.occurrence {
                Occurrence::OneOrMore { .. } => (1, None),
                Occurrence::Bounded { min, max, .. } => (*min, *max),
                _ => (0, None),
            };
            if min > 1 {
                return Err(IrError::Unsupported {
                    rule:   name.into(),
                    reason: format!("member '{}' must appear {min} times, but a map holds a key once", f.name),
                });
            }
            let _ = max;
            let (occ, what) = if min == 1 { (Occurrence::Required, "required") } else { (Occurrence::Optional, "optional") };
            self.warnings.push(IrWarning {
                rule:    Some(name.to_owned()),
                message: format!("member '{}' may repeat, but a map holds a key once: it's {what}", f.name),
                kind:    crate::error::IrWarningKind::RepeatedMapMember,
            });
            f.occurrence = occ;
        }
        let rest = computed.map(|(key_ty, val_ty, occurrence)| {
            let taken = |n: &str| fields.iter().any(|f: &FieldDef| f.name == n);
            let name = if taken("extra") { "extra_entries" } else { "extra" };
            MapRest { name: name.into(), key_ty, val_ty, occurrence }
        });

        Ok(TypeDef::Struct(StructDef {
            name:       name.into(),
            fields,
            encoding:   StructEncoding::Map,
            rest,
            tagged:     meta.tag,
            dcbor:      false,
            doc:        meta.doc.clone(),
            deprecated: meta.deprecated,
        }))
    }

    /// Whether `ty` is `tstr`, directly or through aliases.
    fn is_tstr(&self, ty: &TypeRef, depth: usize) -> bool {
        match ty {
            TypeRef::Primitive(Primitive::Tstr) => true,
            TypeRef::Named(n) if depth <= 8 => matches!(
                self.output.get(n.as_str()),
                Some(TypeDef::Alias(a)) if self.is_tstr(&a.ty, depth + 1)
            ),
            _ => false,
        }
    }

    /// Whether `ty` is text, an integer, or a choice (or enum) of them.
    fn is_scalar_key(&self, ty: &TypeRef, depth: usize) -> bool {
        if depth > 8 {
            return false;
        }
        match ty {
            TypeRef::Primitive(Primitive::Tstr | Primitive::Uint | Primitive::Int) => true,
            TypeRef::Choice(alts) => alts.iter().all(|a| self.is_scalar_key(a, depth + 1)),
            TypeRef::Named(n) => match self.output.get(n.as_str()) {
                Some(TypeDef::Alias(a)) => self.is_scalar_key(&a.ty, depth + 1),
                Some(TypeDef::Enum(e)) => e.variants.iter().all(|v| self.is_scalar_key(&v.ty, depth + 1)),
                _ => false,
            },
            _ => false,
        }
    }

    /// The entries of `choices` with each reference to a group (`g`), unwrapped
    /// map or array (`~base`), group socket (`$$g`) and parenthesized group
    /// replaced by the entries it stands for (RFC 8610 §2.1.2, §3.7, §3.9;
    /// dgtlrift/cddlc#83). In an array, a repeated group (`[* g]`) is left as is.
    fn expand_groups(
        &self,
        rule:     &str,
        choices:  &[GroupChoice],
        in_array: bool,
    ) -> Result<Vec<GroupChoice>, IrError> {
        let mut out = Vec::new();
        for gc in choices {
            for entries in self.expand_entries(rule, &gc.entries, in_array, 0)? {
                out.push(GroupChoice { entries });
            }
        }
        if out.len() > MAX_ALTERNATIVES {
            return Err(IrError::Unsupported {
                rule:   rule.into(),
                reason: format!("more than {MAX_ALTERNATIVES} alternatives from group choices (`//`)"),
            });
        }
        Ok(out)
    }

    /// The alternatives `entries` stands for: one list of entries each. A group
    /// choice (`g = ( a // b )`) used among other entries gives one alternative
    /// per choice, each with the other entries (RFC 8610 §2.2.2;
    /// dgtlrift/cddlc#84).
    fn expand_entries(
        &self,
        rule:     &str,
        entries:  &[Spanned<cddlc_parser::MemberEntry>],
        in_array: bool,
        depth:    usize,
    ) -> Result<Vec<Vec<Spanned<cddlc_parser::MemberEntry>>>, IrError> {
        let unsupported = |reason: &str| IrError::Unsupported { rule: rule.into(), reason: reason.into() };
        if depth > 16 {
            return Err(unsupported("a group that includes itself"));
        }
        let mut alts: Vec<Vec<Spanned<cddlc_parser::MemberEntry>>> = vec![Vec::new()];
        for e in entries {
            let plain = |alts: &mut Vec<Vec<_>>| for a in alts.iter_mut() { a.push(e.clone()) };
            let MemberKind::Type(ty) = &e.node.kind else { plain(&mut alts); continue };
            let group = match single_type2(&ty.node) {
                Some(Type2::Typename { name, args }) if args.is_empty() => {
                    match self.group_alternatives(name.node.as_str()) {
                        Some(g) => g,
                        None => { plain(&mut alts); continue; }
                    }
                }
                Some(Type2::Unwrap(name)) => match self.unwrap_alternatives(name.node.as_str()) {
                    Some(g) => g,
                    None => return Err(unsupported("`~` on a type other than a map or array")),
                },
                Some(Type2::Group(g)) => g.node.choices.iter().map(|c| c.entries.clone()).collect(),
                _ => { plain(&mut alts); continue; }
            };
            let optional = match e.node.occurrence.as_ref().map(|o| &o.node) {
                None => false,
                Some(AstOcc::Optional) => true,
                // A repeated group: an array's is a later fix (dgtlrift/cddlc#90).
                Some(_) if in_array => { plain(&mut alts); continue; }
                Some(_) => return Err(unsupported("a repeated group in a map")),
            };
            // The group's own alternatives, each expanded.
            let mut inner: Vec<Vec<Spanned<cddlc_parser::MemberEntry>>> = Vec::new();
            for g in &group {
                inner.extend(self.expand_entries(rule, g, in_array, depth + 1)?);
            }
            if inner.is_empty() {
                continue; // an empty group socket
            }
            if optional {
                if inner.len() == 1 {
                    // `? g`: each of the group's members is optional.
                    for m in &mut inner[0] {
                        if m.node.occurrence.is_none() {
                            m.node.occurrence = Some(Spanned::new(AstOcc::Optional, m.span));
                        }
                    }
                } else {
                    // `? (a // b)`: none of them, or one.
                    inner.insert(0, Vec::new());
                }
            }
            let mut next = Vec::with_capacity(alts.len() * inner.len());
            for a in &alts {
                for g in &inner {
                    let mut combined = a.clone();
                    combined.extend(g.iter().cloned());
                    next.push(combined);
                }
            }
            if next.len() > MAX_ALTERNATIVES {
                return Err(unsupported("too many alternatives from group choices (`//`)"));
            }
            alts = next;
        }
        Ok(alts)
    }

    /// The constraints a range or control operator puts on `base`. Those that
    /// need the schema (`.within`, `.and`, `.bits`, `.plus`, `.cat`) are lowered
    /// here; the others in [`lower_type_op`] (dgtlrift/cddlc#86).
    fn lower_op(
        &mut self,
        op:    &cddlc_parser::TypeOp,
        base:  &Type2,
        rule:  &str,
        hook:  Option<&str>,
        depth: usize,
    ) -> Result<Vec<Constraint>, IrError> {
        use cddlc_parser::{CtlOp, TypeOp};
        let TypeOp::Control { op: ctl, operand } = op else {
            return lower_type_op(op, base, rule, hook, &mut self.warnings);
        };
        let operand = &operand.node;
        let unchecked = |this: &mut Self, what: String| {
            this.warnings.push(IrWarning {
                rule:    Some(rule.to_owned()),
                message: format!("{what} isn't checked by cddlc validate or generated code"),
                kind:    crate::error::IrWarningKind::UncheckedControlOp,
            });
            Ok(vec![])
        };
        match &ctl.node {
            // `T .within U` / `T .and U` (RFC 8610 §3.8.5): a value of both, so U's
            // constraints too.
            CtlOp::Within | CtlOp::And => {
                let name = if matches!(ctl.node, CtlOp::Within) { ".within" } else { ".and" };
                match self.type_constraints(operand, rule, hook, depth) {
                    Some(cs) => cs,
                    None => unchecked(self, format!("'{name}' with this operand")),
                }
            }
            // `uint .bits flags` (RFC 8610 §3.8.2): the bit numbers the operand's
            // values name are the only ones that may be set.
            CtlOp::Bits => match self.bit_numbers(operand, rule) {
                Some(bits) => Ok(vec![Constraint::Bits(bits)]),
                None => unchecked(self, "'.bits' with this operand".into()),
            },
            // `1 .plus 2` and `"a" .cat "b"` (RFC 9165) compute a literal.
            CtlOp::Other(name) if name == "plus" || name == "cat" => {
                match (base, operand) {
                    (Type2::Value(a), Type2::Value(b)) => match fold_literal(name, a, b) {
                        Some(v) => Ok(vec![Constraint::Eq(v)]),
                        None => Err(IrError::InvalidControlOp {
                            op:   format!(".{name}"),
                            ty:   "operands of these kinds".into(),
                            rule: rule.to_owned(),
                        }),
                    },
                    _ => unchecked(self, format!("'.{name}' on a value other than a literal")),
                }
            }
            // `"a" .det "b"` (RFC 9165 §3): `.cat` of the operands, each dedented.
            CtlOp::Other(name) if name == "det" => match (base, operand) {
                (Type2::Value(Value::Text(a)), Type2::Value(Value::Text(b))) =>
                    Ok(vec![Constraint::Eq(LiteralValue::Text(dedent(a) + &dedent(b)))]),
                (Type2::Value(Value::Bytes(a)), Type2::Value(Value::Bytes(b))) => {
                    let (a, b) = (String::from_utf8_lossy(a), String::from_utf8_lossy(b));
                    Ok(vec![Constraint::Eq(LiteralValue::Bytes((dedent(&a) + &dedent(&b)).into_bytes()))])
                }
                _ => unchecked(self, "'.det' on a value other than a literal".into()),
            },
            // `T .feature "name"` (RFC 9165 §4) is `T`: the feature only names what
            // the schema uses.
            CtlOp::Other(name) if name == "feature" => Ok(vec![]),
            // RFC 9741: text holding bytes, an integer or JSON that matches the
            // operand (`tstr .b64u (bstr .size 32)`).
            CtlOp::Other(name) if TextEncoding::from_control(name).is_some() => {
                let encoding = TextEncoding::from_control(name).expect("checked");
                match self.controller(operand, rule, hook, depth) {
                    Some(Ok((ty, constraints))) => {
                        self.warnings.push(IrWarning {
                            rule:    Some(rule.to_owned()),
                            message: format!("'{}' is checked by cddlc validate, not by generated code", encoding.control()),
                            kind:    crate::error::IrWarningKind::UncheckedControlOp,
                        });
                        Ok(vec![Constraint::TextEncoded { encoding, ty: Box::new(ty), constraints }])
                    }
                    Some(Err(e)) => Err(e),
                    None => unchecked(self, format!("'.{name}' with this operand")),
                }
            }
            CtlOp::Other(name) if name == "abnf" || name == "abnfb" || name == "printf" || name == "join" => {
                unchecked(self, format!("'.{name}'"))
            }
            _ => lower_type_op(op, base, rule, hook, &mut self.warnings),
        }
    }

    /// The type and constraints of a control operator's operand that is itself a
    /// type (`bstr`, `digest`, `(bstr .size 32)`, `1..10`). `None` when cddlc
    /// can't reduce it to one.
    fn controller(
        &mut self,
        t2:    &Type2,
        rule:  &str,
        hook:  Option<&str>,
        depth: usize,
    ) -> Option<Result<(TypeRef, Vec<Constraint>), IrError>> {
        if depth > 8 {
            return None;
        }
        match t2 {
            Type2::Typename { args, .. } if args.is_empty() =>
                Some(crate::lower_constraints::lower_type2_to_typeref(t2, rule).map(|t| (t, vec![]))),
            Type2::Value(v) => {
                let lit = lower_value(v);
                let p = match &lit {
                    LiteralValue::Uint(_) => Primitive::Uint,
                    LiteralValue::Int(_) => Primitive::Int,
                    LiteralValue::Float(_) => Primitive::Float,
                    LiteralValue::Text(_) => Primitive::Tstr,
                    LiteralValue::Bytes(_) => Primitive::Bstr,
                    LiteralValue::Bool(_) => Primitive::Bool,
                    LiteralValue::Null => Primitive::Null,
                };
                Some(Ok((TypeRef::Primitive(p), vec![Constraint::Eq(lit)])))
            }
            Type2::Group(g) => {
                let [gc] = strip_parens(&g.node).choices.as_slice() else { return None };
                let [e] = gc.entries.as_slice() else { return None };
                let MemberKind::Type(t) = &e.node.kind else { return None };
                if e.node.occurrence.is_some() {
                    return None;
                }
                let [t1] = t.node.choices.as_slice() else { return None };
                let t1 = &t1.node;
                let (ty, mut cs) = match self.controller(&t1.base.node, rule, hook, depth + 1)? {
                    Ok(x) => x,
                    Err(e) => return Some(Err(e)),
                };
                // In a range (`0..10`) the literal is a bound, not the value.
                if t1.ops.iter().any(|o| matches!(o, cddlc_parser::TypeOp::Range { .. })) {
                    cs.retain(|c| !matches!(c, Constraint::Eq(_)));
                }
                for op in &t1.ops {
                    match self.lower_op(op, &t1.base.node, rule, hook, depth + 1) {
                        Ok(more) => cs.extend(more),
                        Err(e) => return Some(Err(e)),
                    }
                }
                Some(Ok((ty, cs)))
            }
            _ => None,
        }
    }

    /// The constraints the type `t2` puts on a value: a literal, a parenthesized
    /// or named type with ranges or control operators. `None` when cddlc can't
    /// reduce it to constraints.
    fn type_constraints(
        &mut self,
        t2:    &Type2,
        rule:  &str,
        hook:  Option<&str>,
        depth: usize,
    ) -> Option<Result<Vec<Constraint>, IrError>> {
        if depth > 8 {
            return None;
        }
        let ty: Type = match t2 {
            Type2::Value(v) => return Some(Ok(vec![Constraint::Eq(lower_value(v))])),
            Type2::Group(g) => match strip_parens(&g.node).choices.as_slice() {
                [gc] => match gc.entries.as_slice() {
                    [e] if e.node.occurrence.is_none() => match &e.node.kind {
                        MemberKind::Type(t) => t.node.clone(),
                        _ => return None,
                    },
                    _ => return None,
                },
                _ => return None,
            },
            Type2::Typename { name, args } if args.is_empty() => {
                if Primitive::from_prelude(name.node.as_str()).is_some() {
                    return Some(Ok(vec![])); // `.and uint`: no constraint of its own
                }
                let r = self.module.rules.iter().map(|sr| &sr.node)
                    .find(|r| r.name.node.0 == name.node.0 && r.generic_params.is_empty())?;
                match &r.kind {
                    RuleKind::Type { ty, extend: false } => ty.node.clone(),
                    _ => return None,
                }
            }
            _ => return None,
        };
        let [t1] = ty.choices.as_slice() else { return None };
        let t1 = &t1.node;
        if t1.ops.is_empty() {
            return self.type_constraints(&t1.base.node, rule, hook, depth + 1);
        }
        let mut out = Vec::new();
        for op in &t1.ops {
            match self.lower_op(op, &t1.base.node, rule, hook, depth + 1) {
                Ok(cs) => out.extend(cs),
                Err(e) => return Some(Err(e)),
            }
        }
        Some(Ok(out))
    }

    /// The bit numbers a `.bits` operand names: the unsigned integer values of
    /// a group's members (`&( a: 0, b: 1 )`, `flags` with `flags = &(...)`).
    fn bit_numbers(&self, operand: &Type2, rule: &str) -> Option<Vec<u64>> {
        let values = match operand {
            Type2::EnumGroup(_) | Type2::EnumTypename { .. } => self.enumeration_values(rule, operand).ok()?,
            Type2::Value(Value::Uint(n)) => return Some(vec![*n]),
            Type2::Typename { name, args } if args.is_empty() => {
                let r = self.module.rules.iter().map(|sr| &sr.node)
                    .find(|r| r.name.node.0 == name.node.0 && r.generic_params.is_empty())?;
                match &r.kind {
                    RuleKind::Type { ty, extend: false } => ty.node.clone(),
                    RuleKind::Group { .. } => self.enumeration_values(rule, &Type2::EnumTypename {
                        name: name.clone(),
                        args: vec![],
                    }).ok()?,
                    _ => return None,
                }
            }
            _ => return None,
        };
        let mut bits = Vec::new();
        for c in &values.choices {
            match &c.node.base.node {
                Type2::Value(Value::Uint(n)) if c.node.ops.is_empty() => bits.push(*n),
                Type2::EnumGroup(_) | Type2::EnumTypename { .. } | Type2::Typename { .. } if c.node.ops.is_empty() => {
                    bits.extend(self.bit_numbers(&c.node.base.node, rule)?);
                }
                _ => return None,
            }
        }
        bits.sort_unstable();
        bits.dedup();
        Some(bits)
    }

    /// A named type (`base`, made unique) for an enumeration used in place.
    fn enumeration_type(&mut self, base: &str, t2: &Type2) -> Result<TypeRef, IrError> {
        let mut name: SmolStr = base.into();
        while self.output.contains_key(&name) || self.known_rules.contains(&name) {
            name = format!("{name}-type").into();
        }
        self.known_rules.insert(name.clone());
        let values = self.enumeration_values(&name, t2)?;
        let def = self.lower_type_rule(&name, &values, &PragmaMeta::default())?;
        self.output.insert(name.clone(), def);
        Ok(TypeRef::Named(name))
    }

    /// The type an enumeration (`&colors`, `&( red: 1, green: 2 )`) stands
    /// for: the choice of the values of the group's members (RFC 8610 §3.9).
    fn enumeration_values(&self, rule: &str, t2: &Type2) -> Result<Type, IrError> {
        let unsupported = |reason: String| IrError::Unsupported { rule: rule.into(), reason };
        let choices: Vec<GroupChoice> = match t2 {
            Type2::EnumGroup(g) => g.node.choices.clone(),
            Type2::EnumTypename { name, args } if args.is_empty() => {
                match self.group_alternatives(name.node.as_str()) {
                    Some(alts) => alts.into_iter().map(|entries| GroupChoice { entries }).collect(),
                    None => return Err(unsupported(format!("`&{}`: an enumeration of a type, not a group", name.node.as_str()))),
                }
            }
            _ => return Err(unsupported("an enumeration of a generic group".into())),
        };
        let mut values: Vec<Spanned<Type1>> = Vec::new();
        for gc in self.expand_groups(rule, &choices, false)? {
            for e in &gc.entries {
                let ty = match &e.node.kind {
                    MemberKind::KeyValue { value, .. } => &value.node,
                    MemberKind::Type(t) => &t.node,
                };
                values.extend(ty.choices.iter().cloned());
            }
        }
        if values.is_empty() {
            return Err(unsupported("an enumeration of an empty group".into()));
        }
        Ok(Type { choices: values })
    }

    /// A map or array with a choice between groups (`{ a: uint // b: tstr }`):
    /// a choice between one map or array per alternative, named
    /// `<rule>-alt<n>` (RFC 8610 §2.2.2; dgtlrift/cddlc#84).
    fn lower_alternatives(
        &mut self,
        name:     &str,
        choices:  &[GroupChoice],
        meta:     &PragmaMeta,
        in_array: bool,
    ) -> Result<TypeDef, IrError> {
        let mut variants = Vec::new();
        let tags = if in_array { None } else { discriminants(choices) };
        for (i, gc) in choices.iter().enumerate() {
            // `Command-browser.close` when the alternatives differ by a literal
            // member (`method: "browser.close"`), else `Command-alt1`.
            let variant: SmolStr = match &tags {
                Some(t) => t[i].as_str().into(),
                None => format!("alt{}", i + 1).into(),
            };
            let mut alt: SmolStr = format!("{name}-{variant}").into();
            while self.output.contains_key(&alt) || self.known_rules.contains(&alt) {
                alt = format!("{alt}-type").into();
            }
            self.known_rules.insert(alt.clone());
            let one = std::slice::from_ref(gc);
            let def = if in_array {
                self.lower_array_type(&alt, one, &PragmaMeta::default())?
            } else {
                self.lower_map_as_struct(&alt, one, &PragmaMeta::default())?
            };
            self.output.insert(alt.clone(), def);
            if let Some(src) = self.sources.types.get(name).cloned() {
                self.sources.types.insert(alt.clone(), src);
            }
            variants.push(EnumVariant {
                name:        variant,
                ty:          TypeRef::Named(alt),
                constraints: vec![],
            });
        }
        Ok(TypeDef::Enum(EnumDef {
            name:       name.into(),
            variants,
            tagged:     meta.tag,
            doc:        meta.doc.clone(),
            deprecated: meta.deprecated,
        }))
    }

    /// The alternatives of the group named `name` (each a list of entries), when
    /// it is a group: a group rule, extensions with `//=`, or a group socket
    /// (`$$g`), which may have none. `None` when `name` is a type.
    fn group_alternatives(&self, name: &str) -> Option<Vec<Vec<Spanned<cddlc_parser::MemberEntry>>>> {
        let mut alts = Vec::new();
        let mut found = false;
        for sr in &self.module.rules {
            let r = &sr.node;
            if r.name.node.0 != name || !r.generic_params.is_empty() {
                continue;
            }
            found = true;
            let group = match &r.kind {
                RuleKind::Group { group, .. } => strip_parens(&group.node),
                RuleKind::Type { ty, .. } => match single_type2(&ty.node) {
                    Some(Type2::Group(g)) => strip_parens(&g.node),
                    _ => return None,
                },
            };
            // `t = (int / uint)`: a parenthesized type.
            if let [gc] = group.choices.as_slice() {
                if let [e] = gc.entries.as_slice() {
                    if e.node.occurrence.is_none() {
                        if let MemberKind::Type(t) = &e.node.kind {
                            if !matches!(single_type2(&t.node), Some(Type2::Typename { .. } | Type2::Unwrap(_))) {
                                return None;
                            }
                        }
                    }
                }
            }
            alts.extend(group.choices.iter().map(|c| c.entries.clone()));
        }
        (found || name.starts_with("$$")).then_some(alts)
    }

    /// The alternatives of the map or array type `~name` unwraps.
    fn unwrap_alternatives(&self, name: &str) -> Option<Vec<Vec<Spanned<cddlc_parser::MemberEntry>>>> {
        let rule = self.module.rules.iter().map(|sr| &sr.node)
            .find(|r| r.name.node.0 == name && r.generic_params.is_empty())?;
        let RuleKind::Type { ty, extend: false } = &rule.kind else { return None };
        match single_type2(&ty.node)? {
            Type2::Map(g) | Type2::Array(g) => Some(g.node.choices.iter().map(|c| c.entries.clone()).collect()),
            _ => None,
        }
    }

    /// Lower `{ occ K => V }` to a [`MapDef`].
    fn lower_table(
        &mut self,
        name:   &str,
        entry:  &Spanned<cddlc_parser::MemberEntry>,
        key_ty: &Type,
        val_ty: &Type,
        meta:   &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        let (key_ty, val_ty, occurrence) = self.lower_computed_entry(name, entry, key_ty, val_ty, meta)?;
        Ok(TypeDef::Map(MapDef {
            name:       name.into(),
            key_ty,
            val_ty,
            occurrence,
            tagged:     meta.tag,
            doc:        meta.doc.clone(),
            deprecated: meta.deprecated,
        }))
    }

    /// The key type, value type and occurrence of a member with a computed key
    /// (`* tstr => uint`).
    fn lower_computed_entry(
        &mut self,
        name:   &str,
        entry:  &Spanned<cddlc_parser::MemberEntry>,
        key_ty: &Type,
        val_ty: &Type,
        meta:   &PragmaMeta,
    ) -> Result<(TypeRef, TypeRef, Occurrence), IrError> {
        let me = &entry.node;
        // A `; @capacity` on the member wins over one on the rule.
        let mut occ_meta = PragmaMeta::from_pragmas(&me.pragmas);
        if occ_meta.capacity.is_none() {
            occ_meta.capacity = meta.capacity;
        }
        let occurrence = self.lower_occurrence(
            me.occurrence.as_ref().map(|o| &o.node),
            name,
            &occ_meta,
        );
        // A parenthesised key, `* (int / tstr) => any` (RFC 9203), is the type
        // it wraps.
        let key_ty = paren_type(key_ty).unwrap_or(key_ty);
        // An inline choice of key types, `* (int / tstr) => any`, is an enum
        // of its own (`<rule>-key`), as a named `label = int / tstr` would be,
        // so every backend holds each alternative.
        let key_ty = if key_ty.choices.len() > 1 && key_ty.choices.iter().all(|c| c.node.ops.is_empty()) {
            let mut ename: SmolStr = format!("{name}-key").into();
            while self.output.contains_key(&ename) || self.known_rules.contains(&ename) {
                ename = format!("{ename}-type").into();
            }
            let mut variants = Vec::new();
            for (i, choice) in key_ty.choices.iter().enumerate() {
                let (var_name, ty, constraints) = self.classify_enum_variant(&choice.node.base.node, i, &ename)?;
                variants.push(EnumVariant { name: var_name, ty, constraints });
            }
            self.known_rules.insert(ename.clone());
            self.output.insert(ename.clone(), TypeDef::Enum(EnumDef {
                name:       ename.clone(),
                variants,
                tagged:     None,
                doc:        None,
                deprecated: false,
            }));
            TypeRef::Named(ename)
        } else {
            let (key_ty, key_cs) = self.lower_type_to_ref(key_ty, name)?;
            self.constrained_alias(name, "key", key_ty, key_cs)
        };
        let (val_ty, val_cs) = self.lower_field_type(val_ty, name, "value")?;
        let val_ty = self.constrained_alias(name, "value", val_ty, val_cs);
        Ok((key_ty, val_ty, occurrence))
    }

    /// A map key or value type with control operators (`{ * tstr => uint .le 9 }`)
    /// becomes a named alias (`<rule>-value`) that carries them, since a
    /// [`MapDef`] holds bare types.
    fn constrained_alias(
        &mut self,
        rule:        &str,
        part:        &str,
        ty:          TypeRef,
        constraints: Vec<Constraint>,
    ) -> TypeRef {
        if constraints.is_empty() {
            return ty;
        }
        let mut name: SmolStr = format!("{rule}-{part}").into();
        while self.output.contains_key(&name) || self.known_rules.contains(&name) {
            name = format!("{name}-type").into();
        }
        self.known_rules.insert(name.clone());
        self.output.insert(name.clone(), TypeDef::Alias(AliasDef {
            name:        name.clone(),
            ty,
            constraints,
            tagged:      None,
            doc:         None,
            deprecated:  false,
        }));
        TypeRef::Named(name)
    }

    fn lower_group_as_struct(
        &mut self,
        name:    &str,
        choices: &[GroupChoice],
        meta:    &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        // Used as a type, a group rule is the map holding its members, which
        // can include computed keys (`Extensible = (* text => any)`).
        self.lower_map_as_struct(name, choices, meta)
    }

    fn lower_member_entry(
        &mut self,
        entry:     &Spanned<cddlc_parser::MemberEntry>,
        rule_name: &str,
    ) -> Result<FieldDef, IrError> {
        let me = &entry.node;

        // Occurrence
        let occurrence = self.lower_occurrence(
            me.occurrence.as_ref().map(|o| &o.node),
            rule_name,
            &PragmaMeta::from_pragmas(&me.pragmas),
        );

        match &me.kind {
            // In a group or record, a computed key can't be a named member.
            MemberKind::KeyValue { key, .. }
                if matches!(key.node, cddlc_parser::MemberKey::Type(_)) =>
            {
                Err(IrError::Unsupported {
                    rule:   rule_name.into(),
                    reason: "a member with a computed key outside a map".into(),
                })
            }
            MemberKind::KeyValue { key, value, .. } => {
                let field_name: SmolStr = match &key.node {
                    cddlc_parser::MemberKey::Bareword(id) => id.0.clone(),
                    cddlc_parser::MemberKey::Value(Value::Text(s)) => s.as_str().into(),
                    cddlc_parser::MemberKey::Value(Value::Uint(n)) => format!("field_{n}").into(),
                    // `field_minus1` for `-1`: `field_-1` would read as `field_1`
                    // once the `-` is dropped (dgtlrift/cddlc#176).
                    cddlc_parser::MemberKey::Value(Value::Int(n)) if *n < 0 => format!("field_minus{}", n.unsigned_abs()).into(),
                    cddlc_parser::MemberKey::Value(Value::Int(n))  => format!("field_{n}").into(),
                    _ => "unknown_key".into(),
                };

                let field_key = match &key.node {
                    cddlc_parser::MemberKey::Bareword(id) => FieldKey::Text(id.0.clone()),
                    cddlc_parser::MemberKey::Value(Value::Text(s)) => FieldKey::Text(s.as_str().into()),
                    cddlc_parser::MemberKey::Value(Value::Uint(n)) => FieldKey::Uint(*n),
                    cddlc_parser::MemberKey::Value(Value::Int(n))  => FieldKey::Int(*n),
                    _ => FieldKey::Text(field_name.clone()),
                };

                let (ty, constraints) = self.lower_field_type(
                    &value.node, rule_name, &field_name,
                )?;
                if let Some(s) = &me.source {
                    self.sources.fields.insert(
                        (rule_name.into(), field_name.clone()),
                        s.into(),
                    );
                    // A type made from this inline field is where the field is.
                    if let TypeRef::Named(n) = &ty {
                        if !self.sources.types.contains_key(n)
                            && n.starts_with(&format!("{rule_name}-{field_name}"))
                        {
                            self.sources.types.insert(n.clone(), s.into());
                        }
                    }
                }

                Ok(FieldDef {
                    name:        field_name,
                    key:         field_key,
                    ty,
                    occurrence,
                    constraints,
                    default:     None,
                    doc:         me.doc.clone(),
                    deprecated:  false,
                })
            }

            MemberKind::Type(ty) => {
                // Bare type embedded in group — used for group references
                let (tr, constraints) = self.lower_type_to_ref(&ty.node, rule_name)?;
                let field_name: SmolStr = match &tr {
                    TypeRef::Named(n) => n.clone(),
                    TypeRef::Primitive(p) => format!("{p:?}").to_lowercase().into(),
                    _ => "value".into(),
                };
                if let Some(s) = &me.source {
                    self.sources.fields.insert(
                        (rule_name.into(), field_name.clone()),
                        s.into(),
                    );
                }
                Ok(FieldDef {
                    name:        field_name,
                    key:         FieldKey::Text("value".into()),
                    ty:          tr,
                    occurrence,
                    constraints,
                    default:     None,
                    doc:         me.doc.clone(),
                    deprecated:  false,
                })
            }
        }
    }

    // ── Array type ────────────────────────────────────────────────────────────

    fn lower_array_type(
        &mut self,
        name:    &str,
        choices: &[GroupChoice],
        meta:    &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        let expanded = self.expand_groups(name, choices, true)?;
        if expanded.len() > 1 {
            return self.lower_alternatives(name, &expanded, meta, true);
        }
        let choices = expanded.as_slice();
        // Detect homogeneous array: single entry with occurrence
        let entries: Vec<_> = choices.iter()
            .flat_map(|gc| gc.entries.iter())
            .collect();

        if entries.len() == 1 {
            let entry = &entries[0].node;
            let occ = self.lower_occurrence(
                entry.occurrence.as_ref().map(|o| &o.node),
                name,
                &PragmaMeta::from_pragmas_with_capacity(
                    &entry.pragmas,
                    meta.capacity,
                    meta.str_capacity,
                ),
            );

            if occ.is_repeated() {
                if let Some(mut members) = self.repeated_group(name, entry)? {
                    if members.len() > 1 {
                        return self.lower_flat_array(name, members, occ, meta);
                    }
                    // `[* (a: uint)]` is `[* uint]`.
                    let mut only = members.remove(0);
                    only.node.occurrence = entry.occurrence.clone();
                    return self.lower_array_type(name, &[GroupChoice { entries: vec![only] }], meta);
                }
            }

            let ty = match &entry.kind {
                MemberKind::Type(t) => {
                    let (tr, _) = self.lower_type_to_ref(&t.node, name)?;
                    tr
                }
                MemberKind::KeyValue { value, .. } => {
                    let (tr, _) = self.lower_type_to_ref(&value.node, name)?;
                    tr
                }
            };

            return Ok(TypeDef::Array(ArrayDef {
                name:       name.into(),
                element:    ty,
                occurrence: occ,
                flat:       false,
                tagged:     meta.tag,
                doc:        meta.doc.clone(),
                deprecated: meta.deprecated,
            }));
        }

        // Multi-entry array → a record: a struct encoded as an array
        // (RFC 8610 §3.4), its members positional.
        let mut def = self.lower_struct(name, choices, meta, false)?;
        if let TypeDef::Struct(st) = &mut def {
            st.encoding = StructEncoding::Array;
            // A member without a key (`[uint, tstr]`) is named after its type
            // when that is a rule, else by position; names stay unique.
            let mut seen = std::collections::HashSet::new();
            for (i, (f, e)) in st.fields.iter_mut().zip(&entries).enumerate() {
                if let MemberKind::Type(_) = &e.node.kind {
                    let named = match &f.ty {
                        TypeRef::Named(n) if !n.starts_with(&format!("{name}-")) => Some(n.clone()),
                        _ => None,
                    };
                    f.name = named.unwrap_or_else(|| format!("item{i}").into());
                }
                if !seen.insert(f.name.clone()) {
                    f.name = format!("{}{i}", f.name).into();
                    seen.insert(f.name.clone());
                }
                f.key = FieldKey::Text(f.name.clone());
            }
            // The array's length says which members it holds: an array of
            // n items holds the first n - required optional members, in order
            // (COSE's `Sig_structure` and `Countersign_structure`,
            // dgtlrift/cddlc#134). One repeated member, with every other one
            // required, holds the items that are left (RFC 9171's `bundle =
            // [primary-block, * extension-block, payload-block]`). With more
            // than one, or an optional member beside it, the length doesn't
            // say which items are whose.
            let repeated = st.fields.iter().filter(|f| f.occurrence.is_repeated()).count();
            let optional = st.fields.iter().filter(|f| matches!(f.occurrence, Occurrence::Optional)).count();
            if repeated > 1 || (repeated == 1 && optional > 0) {
                return Err(IrError::Unsupported {
                    rule:   name.into(),
                    reason: "a record array can't repeat a member beside another repeated or optional one yet \
                             (`[? a: uint, * tstr]`, dgtlrift/cddlc#134); use a separate array for the repeated part".into(),
                });
            }
        }
        Ok(def)
    }

    /// The members of a group repeated in an array (`[* (a: uint, b: tstr)]`,
    /// `[+ pair]` for a group rule `pair`); `None` for anything else.
    fn repeated_group(
        &self,
        rule:  &str,
        entry: &cddlc_parser::MemberEntry,
    ) -> Result<Option<Vec<Spanned<cddlc_parser::MemberEntry>>>, IrError> {
        let MemberKind::Type(ty) = &entry.kind else { return Ok(None) };
        let alts = match single_type2(&ty.node) {
            Some(Type2::Group(g)) => strip_parens(&g.node).choices.iter().map(|c| c.entries.clone()).collect(),
            Some(Type2::Typename { name, args }) if args.is_empty() => {
                match self.group_alternatives(name.node.as_str()) {
                    Some(g) => g,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        let alts = self.expand_groups(rule, &alts.into_iter().map(|entries| GroupChoice { entries }).collect::<Vec<_>>(), true)?;
        let [only] = alts.as_slice() else {
            return Err(IrError::Unsupported {
                rule:   rule.into(),
                reason: "a repeated group with choices (`[* (a: uint // b: tstr)]`)".into(),
            });
        };
        if only.entries.is_empty() {
            return Ok(None);
        }
        if only.entries.iter().any(|e| e.node.occurrence.is_some()) {
            return Err(IrError::Unsupported {
                rule:   rule.into(),
                reason: "a repeated group whose members are optional or repeated \
                         (`[* (a: uint, ? b: tstr)]`); each repetition must have the same members".into(),
            });
        }
        Ok(Some(only.entries.clone()))
    }

    /// An array of a repeated group: an array of `<rule>-entry` records whose
    /// members sit inline (dgtlrift/cddlc#90).
    fn lower_flat_array(
        &mut self,
        name:    &str,
        members: Vec<Spanned<cddlc_parser::MemberEntry>>,
        occ:     Occurrence,
        meta:    &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        let mut entry: SmolStr = format!("{name}-entry").into();
        while self.output.contains_key(&entry) || self.known_rules.contains(&entry) {
            entry = format!("{entry}-type").into();
        }
        self.known_rules.insert(entry.clone());
        let def = self.lower_array_type(&entry, &[GroupChoice { entries: members }], &PragmaMeta::default())?;
        if !matches!(&def, TypeDef::Struct(s) if s.encoding == StructEncoding::Array) {
            return Err(IrError::Unsupported {
                rule:   name.into(),
                reason: "a repeated group that is not a list of members".into(),
            });
        }
        self.output.insert(entry.clone(), def);
        Ok(TypeDef::Array(ArrayDef {
            name:       name.into(),
            element:    TypeRef::Named(entry),
            occurrence: occ,
            flat:       true,
            tagged:     meta.tag,
            doc:        meta.doc.clone(),
            deprecated: meta.deprecated,
        }))
    }

    // ── Tagged type ───────────────────────────────────────────────────────────

    fn lower_tagged_type(
        &mut self,
        name: &str,
        ty:   &cddlc_parser::Type,
        meta: &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        // Recurse — the tag is already in meta
        self.lower_type_rule(name, ty, meta)
    }

    // ── Alias ─────────────────────────────────────────────────────────────────

    fn lower_alias(
        &mut self,
        name:     &str,
        ref_name: &str,
        t1:       &Type1,
        meta:     &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        let tr = self.resolve_typeref(ref_name, name)?;

        // Gather constraints from control operators
        let mut constraints: Vec<Constraint> = Vec::new();
        // The prelude's `nint` is `#1`, a negative integer.
        if ref_name == "nint" {
            constraints.extend(major_type(1, None).1);
        }
        for op in &t1.ops {
            let cs = self.lower_op(op, &t1.base.node, name, meta.regex_hook.as_deref(), 0)?;
            constraints.extend(cs);
        }

        Ok(TypeDef::Alias(AliasDef {
            name:        name.into(),
            ty:          tr,
            constraints,
            tagged:      meta.tag,
            doc:         meta.doc.clone(),
            deprecated:  meta.deprecated,
        }))
    }

    // ── Generic instantiation ─────────────────────────────────────────────────

    fn lower_generic_instance(
        &mut self,
        instance_name: &str,
        generic_name:  &str,
        args:          &[Spanned<Type>],
        meta:          &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        let (params, template_rule) = self
            .generics
            .get(generic_name)
            .map(|(p, r)| (p.clone(), *r))
            .ok_or_else(|| if self.known_rules.contains(generic_name) {
                IrError::NotGeneric { name: generic_name.to_owned() }
            } else {
                IrError::UndefinedType { name: generic_name.to_owned(), in_rule: instance_name.to_owned() }
            })?;

        if params.len() != args.len() {
            return Err(IrError::GenericArgCount {
                name:     generic_name.to_owned(),
                expected: params.len(),
                got:      args.len(),
            });
        }

        // Build substitution map: param name → concrete TypeRef
        let mut subst: HashMap<SmolStr, TypeRef> = HashMap::new();
        for (param, arg) in params.iter().zip(args.iter()) {
            let tr = self.lower_type_single(&arg.node, instance_name)?;
            subst.insert(param.clone(), tr);
        }

        // Substitute into the template rule and lower it under the instance name
        self.lower_rule_with_subst(instance_name, template_rule, &subst, meta)
    }

    fn lower_rule_with_subst(
        &mut self,
        name:     &str,
        rule:     &Rule,
        subst:    &HashMap<SmolStr, TypeRef>,
        meta:     &PragmaMeta,
    ) -> Result<TypeDef, IrError> {
        match &rule.kind {
            RuleKind::Type { ty, .. } => {
                let substituted = self.substitute_type(&ty.node, subst);
                self.lower_type_rule(name, &substituted, meta)
            }
            RuleKind::Group { group, .. } => {
                // For group rules with substitution, lower as struct
                // (substitution in group entries is handled field by field)
                let fields: Result<Vec<FieldDef>, _> = group.node.choices.iter()
                    .flat_map(|gc| gc.entries.iter())
                    .map(|e| self.lower_member_entry_with_subst(e, name, subst))
                    .collect();

                Ok(TypeDef::Struct(StructDef {
                    name:       name.into(),
                    fields:     fields?,
                    encoding:   StructEncoding::Map,
                    rest:       None,
                    tagged:     meta.tag,
                    dcbor:      false,
                    doc:        meta.doc.clone(),
                    deprecated: meta.deprecated,
                }))
            }
        }
    }

    fn lower_member_entry_with_subst(
        &mut self,
        entry:  &Spanned<cddlc_parser::MemberEntry>,
        rule_name: &str,
        subst:  &HashMap<SmolStr, TypeRef>,
    ) -> Result<FieldDef, IrError> {
        let me = &entry.node;
        let occurrence = self.lower_occurrence(
            me.occurrence.as_ref().map(|o| &o.node),
            rule_name,
            &PragmaMeta::default(),
        );

        match &me.kind {
            MemberKind::KeyValue { key, .. }
                if matches!(key.node, cddlc_parser::MemberKey::Type(_)) =>
            {
                Err(IrError::Unsupported {
                    rule:   rule_name.into(),
                    reason: "a member with a computed key in a generic group".into(),
                })
            }
            MemberKind::KeyValue { key, value, .. } => {
                let field_name: SmolStr = match &key.node {
                    cddlc_parser::MemberKey::Bareword(id) => id.0.clone(),
                    cddlc_parser::MemberKey::Value(Value::Text(s)) => s.as_str().into(),
                    _ => "field".into(),
                };
                let field_key = match &key.node {
                    cddlc_parser::MemberKey::Bareword(id) => FieldKey::Text(id.0.clone()),
                    cddlc_parser::MemberKey::Value(Value::Text(s)) => FieldKey::Text(s.as_str().into()),
                    cddlc_parser::MemberKey::Value(Value::Uint(n)) => FieldKey::Uint(*n),
                    cddlc_parser::MemberKey::Value(Value::Int(n))  => FieldKey::Int(*n),
                    _ => FieldKey::Text(field_name.clone()),
                };

                // Substitute type parameters
                let subst_type = self.substitute_type(&value.node, subst);
                let (ty, constraints) = self.lower_field_type(&subst_type, rule_name, &field_name)?;

                Ok(FieldDef {
                    name: field_name, key: field_key, ty,
                    occurrence, constraints,
                    default: None, doc: None, deprecated: false,
                })
            }
            MemberKind::Type(ty_node) => {
                let subst_type = self.substitute_type(&ty_node.node, subst);
                let (tr, constraints) = self.lower_type_to_ref(&subst_type, rule_name)?;
                let field_name: SmolStr = match &tr {
                    TypeRef::Named(n) => n.clone(),
                    _ => "value".into(),
                };
                Ok(FieldDef {
                    name: field_name.clone(),
                    key: FieldKey::Text(field_name),
                    ty: tr, occurrence, constraints,
                    default: None, doc: None, deprecated: false,
                })
            }
        }
    }

    // ── Type substitution (for generics) ──────────────────────────────────────

    fn substitute_type(&self, ty: &Type, subst: &HashMap<SmolStr, TypeRef>) -> Type {
        Type {
            choices: ty.choices.iter().map(|c| {
                Spanned::new(
                    cddlc_parser::Type1 {
                        base: self.substitute_type2(&c.node.base, subst),
                        ops:  c.node.ops.clone(),
                    },
                    c.span,
                )
            }).collect()
        }
    }

    fn substitute_type2(
        &self,
        t2:    &Spanned<Type2>,
        subst: &HashMap<SmolStr, TypeRef>,
    ) -> Spanned<Type2> {
        match &t2.node {
            // Direct type parameter reference — substitute it
            Type2::Typename { name, args } if args.is_empty() => {
                if let Some(replacement) = subst.get(name.node.as_str()) {
                    if let TypeRef::Primitive(p) = replacement {
                        let new_name = primitive_to_prelude_name(p);
                        return Spanned::new(
                            Type2::Typename {
                                name: Spanned::new(
                                    cddlc_parser::Ident::new(new_name),
                                    name.span,
                                ),
                                args: vec![],
                            },
                            t2.span,
                        );
                    }
                    if let TypeRef::Named(n) = replacement {
                        return Spanned::new(
                            Type2::Typename {
                                name: Spanned::new(
                                    cddlc_parser::Ident::new(n.as_str()),
                                    name.span,
                                ),
                                args: vec![],
                            },
                            t2.span,
                        );
                    }
                }
                t2.clone()
            }

            // Recurse into map/array/group — substitute through member entries
            Type2::Map(group) => {
                let new_group = self.substitute_group(&group.node, subst);
                Spanned::new(Type2::Map(Spanned::new(new_group, group.span)), t2.span)
            }
            Type2::Array(group) => {
                let new_group = self.substitute_group(&group.node, subst);
                Spanned::new(Type2::Array(Spanned::new(new_group, group.span)), t2.span)
            }
            Type2::Group(group) => {
                let new_group = self.substitute_group(&group.node, subst);
                Spanned::new(Type2::Group(Spanned::new(new_group, group.span)), t2.span)
            }

            _ => t2.clone(),
        }
    }

    fn substitute_group(
        &self,
        group: &cddlc_parser::GroupEntry,
        subst: &HashMap<SmolStr, TypeRef>,
    ) -> cddlc_parser::GroupEntry {
        use cddlc_parser::{GroupChoice, GroupEntry, MemberEntry, MemberKind};

        let choices = group.choices.iter().map(|gc| {
            let entries = gc.entries.iter().map(|e| {
                let me = &e.node;
                let new_kind = match &me.kind {
                    MemberKind::KeyValue { key, value, cut } => {
                        let new_value = Spanned::new(
                            self.substitute_type(&value.node, subst),
                            value.span,
                        );
                        MemberKind::KeyValue {
                            key:   key.clone(),
                            value: new_value,
                            cut:   *cut,
                        }
                    }
                    MemberKind::Type(ty) => {
                        MemberKind::Type(Spanned::new(
                            self.substitute_type(&ty.node, subst),
                            ty.span,
                        ))
                    }
                };
                Spanned::new(
                    MemberEntry {
                        kind:       new_kind,
                        occurrence: me.occurrence.clone(),
                        pragmas:    me.pragmas.clone(),
                        doc:        me.doc.clone(),
                        source:     me.source.clone(),
                    },
                    e.span,
                )
            }).collect();
            GroupChoice { entries }
        }).collect();
        GroupEntry { choices }
    }

    // ── Extend rules ──────────────────────────────────────────────────────────

    fn lower_type_extend(&mut self, name: &str, ty: &Type) -> Result<(), IrError> {
        // `/=` extension: add new variants to an existing enum
        let new_variants = ty.choices.iter()
            .enumerate()
            .map(|(i, c)| self.classify_enum_variant(&c.node.base.node, i, name))
            .collect::<Result<Vec<_>, _>>()?;

        match self.output.get_mut(name) {
            Some(TypeDef::Enum(e)) => {
                for (vname, vty, vcs) in new_variants {
                    e.variants.push(EnumVariant { name: vname, ty: vty, constraints: vcs });
                }
                Ok(())
            }
            Some(_) => Err(IrError::IncompatibleExtension {
                name:   name.to_owned(),
                reason: "/= extension requires the base rule to be a type choice".into(),
            }),
            None => {
                // Base rule not yet defined — create it as an enum
                let variants = new_variants.into_iter()
                    .map(|(n, t, cs)| EnumVariant { name: n, ty: t, constraints: cs })
                    .collect();
                self.output.insert(name.into(), TypeDef::Enum(EnumDef {
                    name:       name.into(),
                    variants,
                    tagged:     None,
                    doc:        None,
                    deprecated: false,
                }));
                Ok(())
            }
        }
    }

    fn lower_group_extend(&mut self, name: &str, choices: &[GroupChoice]) -> Result<(), IrError> {
        let expanded = self.expand_groups(name, choices, false)?;
        if expanded.len() > 1 {
            return Err(IrError::Unsupported {
                rule:   name.into(),
                reason: "a `//=` extension with a choice between groups".into(),
            });
        }
        let choices = expanded.as_slice();
        // `//=` extension: add new fields to an existing struct
        let new_fields: Vec<FieldDef> = choices.iter()
            .flat_map(|gc| gc.entries.iter())
            .map(|e| self.lower_member_entry(e, name))
            .collect::<Result<Vec<_>, _>>()?;

        match self.output.get_mut(name) {
            Some(TypeDef::Struct(s)) => {
                s.fields.extend(new_fields);
                Ok(())
            }
            Some(_) => Err(IrError::IncompatibleExtension {
                name:   name.to_owned(),
                reason: "//= extension requires the base rule to be a map/struct".into(),
            }),
            None => {
                self.output.insert(name.into(), TypeDef::Struct(StructDef {
                    name:       name.into(),
                    fields:     new_fields,
                    encoding:   StructEncoding::Map,
                    rest:       None,
                    tagged:     None,
                    dcbor:      false,
                    doc:        None,
                    deprecated: false,
                }));
                Ok(())
            }
        }
    }

    // ── Type reference resolution ─────────────────────────────────────────────

    /// Resolve a type name to a TypeRef.
    fn resolve_typeref(&self, name: &str, in_rule: &str) -> Result<TypeRef, IrError> {
        // `nint` is `#1`, a negative integer (RFC 8610 Appendix D).
        if name == "nint" {
            return Ok(major_type(1, None).0);
        }
        if let Some(p) = Primitive::from_prelude(name) {
            return Ok(TypeRef::Primitive(p));
        }
        if let Some(tr) = crate::prelude::prelude_typeref(name) {
            return Ok(tr);
        }
        if is_prelude(name) {
            return Ok(TypeRef::Primitive(Primitive::Any));
        }
        if self.known_rules.contains(name) {
            return Ok(TypeRef::Named(name.into()));
        }
        Err(IrError::UndefinedType {
            name:    name.to_owned(),
            in_rule: in_rule.to_owned(),
        })
    }

    /// Lower a struct field's value type. An inline choice made only of literals
    /// (`reason: "until" / "stopped"`) becomes a synthesized named enum
    /// (`<rule>-<field>`), so the allowed values survive lowering instead of
    /// collapsing into `TypeRef::Choice` of bare primitives.
    fn lower_field_type(
        &mut self,
        ty:         &Type,
        rule_name:  &str,
        field_name: &str,
    ) -> Result<(TypeRef, Vec<Constraint>), IrError> {
        // A parenthesized type (`kind: ("a" / "b")`) is the type inside it
        // (RFC 8610 §3.1); the parser reads `( ... )` as an inline group.
        if let Some(inner) = paren_type(ty) {
            return self.lower_field_type(inner, rule_name, field_name);
        }
        // An inline array or map (`names: [* tstr]`, `pos: { x: int, y: int }`) becomes a
        // synthesized named type (`<rule>-<field>`), lowered as a rule of that shape would
        // be. Lowered through `lower_type_to_ref` it would be `Primitive::Any`, which the
        // backends emit as bytes (dgtlrift/cddlc#6).
        let inline = match ty.choices.as_slice() {
            [only] if only.node.ops.is_empty() => match &only.node.base.node {
                Type2::Array(group) => Some((true, group)),
                Type2::Map(group) => Some((false, group)),
                _ => None,
            },
            _ => None,
        };
        // A member's enumeration (`c: &colors`) is named for the member.
        if let [only] = ty.choices.as_slice() {
            if only.node.ops.is_empty()
                && matches!(only.node.base.node, Type2::EnumTypename { .. } | Type2::EnumGroup(_))
            {
                let tr = self.enumeration_type(&format!("{rule_name}-{field_name}"), &only.node.base.node)?;
                return Ok((tr, vec![]));
            }
        }
        if let Some((is_array, group)) = inline {
            // `{ g }` for a group rule `g`: one type, shared by every use.
            let only_group = match group.node.choices.as_slice() {
                [gc] => match gc.entries.as_slice() {
                    [e] if e.node.occurrence.is_none() => match &e.node.kind {
                        MemberKind::Type(t) => match single_type2(&t.node) {
                            Some(Type2::Typename { name, args }) if args.is_empty()
                                && self.group_alternatives(name.node.as_str()).is_some() =>
                                Some(SmolStr::from(name.node.as_str())),
                            _ => None,
                        },
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            };
            if let Some(g) = &only_group {
                if let Some(done) = self.inline_groups.get(&(is_array, g.clone())) {
                    return Ok((TypeRef::Named(done.clone()), vec![]));
                }
            }
            let mut name: SmolStr = format!("{rule_name}-{field_name}").into();
            while self.output.contains_key(&name) || self.known_rules.contains(&name) {
                name = format!("{name}-type").into();
            }
            self.known_rules.insert(name.clone());
            if let Some(g) = only_group {
                self.inline_groups.insert((is_array, g), name.clone());
            }
            let meta = PragmaMeta::default();
            let def = if is_array {
                self.lower_array_type(&name, &group.node.choices, &meta)?
            } else {
                self.lower_map_as_struct(&name, &group.node.choices, &meta)?
            };
            self.output.insert(name.clone(), def);
            return Ok((TypeRef::Named(name), vec![]));
        }
        let all_literals = ty.choices.len() > 1
            && ty.choices.iter().all(|c| {
                c.node.ops.is_empty()
                    && matches!(
                        c.node.base.node,
                        Type2::Value(Value::Text(_) | Value::Uint(_) | Value::Int(_))
                    )
            });
        if !all_literals {
            return self.lower_type_to_ref(ty, rule_name);
        }
        let mut name: SmolStr = format!("{rule_name}-{field_name}").into();
        while self.output.contains_key(&name) || self.known_rules.contains(&name) {
            name = format!("{name}-choice").into();
        }
        let mut variants = Vec::new();
        for (i, choice) in ty.choices.iter().enumerate() {
            let (var_name, type_ref, constraints) =
                self.classify_enum_variant(&choice.node.base.node, i, &name)?;
            variants.push(EnumVariant { name: var_name, ty: type_ref, constraints });
        }
        self.known_rules.insert(name.clone());
        self.output.insert(name.clone(), TypeDef::Enum(EnumDef {
            name:       name.clone(),
            variants,
            tagged:     None,
            doc:        None,
            deprecated: false,
        }));
        Ok((TypeRef::Named(name), vec![]))
    }

    /// Lower a full `Type` expression to a single TypeRef + constraints.
    fn lower_type_to_ref(
        &mut self,
        ty:        &Type,
        rule_name: &str,
    ) -> Result<(TypeRef, Vec<Constraint>), IrError> {
        if ty.choices.len() > 1 {
            // Inline choice — produce TypeRef::Choice
            let refs: Vec<TypeRef> = ty.choices.iter()
                .map(|c| self.lower_type2_ref(&c.node.base.node, rule_name))
                .collect::<Result<Vec<TypeRef>, IrError>>()?;
            return Ok((TypeRef::Choice(refs), vec![]));
        }

        let t1 = &ty.choices[0].node;
        let tr = self.lower_type2_ref(&t1.base.node, rule_name)?;

        let mut constraints = Vec::new();
        // A bare literal (`cat: "kernel"`, `version: 1`) only admits that value;
        // keep it as an `.eq` constraint so backends and the validator can enforce it.
        if let Type2::Value(v) = &t1.base.node {
            if t1.ops.is_empty() {
                constraints.push(Constraint::Eq(lower_value(v)));
            }
        }
        // A major type's constraints (`#1` is a negative integer).
        if let Type2::MajorType(mt, ai) = &t1.base.node {
            constraints.extend(major_type(*mt, *ai).1);
        }
        // The prelude's `nint` is `#1`.
        if matches!(&t1.base.node, Type2::Typename { name, args } if args.is_empty() && name.node.as_str() == "nint") {
            constraints.extend(major_type(1, None).1);
        }
        // So are the prelude's `true` and `false` (dgtlrift/cddlc#15).
        if let Some(b) = bool_value_name(&t1.base.node) {
            if t1.ops.is_empty() {
                constraints.push(Constraint::Eq(LiteralValue::Bool(b)));
            }
        }
        for op in &t1.ops {
            let cs = self.lower_op(op, &t1.base.node, rule_name, None, 0)?;
            constraints.extend(cs);
        }

        Ok((tr, constraints))
    }

    fn lower_type2_ref(&mut self, t2: &Type2, rule_name: &str) -> Result<TypeRef, IrError> {
        match t2 {
            Type2::Typename { name, args } if args.is_empty() => {
                self.resolve_typeref(name.node.as_str(), rule_name)
            }
            Type2::Typename { name, args } => {
                // Generic application — instantiate inline
                let instance_name: SmolStr = format!(
                    "{}_{}",
                    name.node.as_str(),
                    args.iter()
                        .map(|a| type_to_name(&a.node))
                        .collect::<Vec<_>>()
                        .join("_")
                ).into();

                if !self.output.contains_key(&instance_name)
                    && !self.known_rules.contains(&instance_name)
                {
                    let meta = PragmaMeta::default();
                    let def = self.lower_generic_instance(
                        &instance_name, name.node.as_str(), args, &meta,
                    )?;
                    self.output.insert(instance_name.clone(), def);
                    self.known_rules.insert(instance_name.clone());
                }
                Ok(TypeRef::Named(instance_name))
            }
            Type2::Value(v) => Ok(literal_to_typeref(&lower_value(v))),
            Type2::Tagged { tag, inner } => {
                let inner_tr = self.lower_type_single(&inner.node, rule_name)?;
                Ok(TypeRef::Tagged(CborTag(*tag), Box::new(inner_tr)))
            }
            Type2::MajorType(mt, ai) => Ok(major_type(*mt, *ai).0),
            Type2::Any => Ok(TypeRef::Primitive(Primitive::Any)),
            Type2::Unwrap(id) => self.resolve_typeref(id.node.as_str(), rule_name),
            Type2::Map(_) | Type2::Array(_) | Type2::Group(_) => {
                // Inline complex type — generate a synthetic named type
                Ok(TypeRef::Primitive(Primitive::Any))
            }
            // An enumeration (`&colors`): a type of its own, the choice of the
            // group's member values (dgtlrift/cddlc#85).
            Type2::EnumTypename { .. } | Type2::EnumGroup(_) => {
                self.enumeration_type(&format!("{rule_name}-enum"), t2)
            }
        }
    }

    fn lower_type_single(&mut self, ty: &Type, rule_name: &str) -> Result<TypeRef, IrError> {
        let (tr, _) = self.lower_type_to_ref(ty, rule_name)?;
        Ok(tr)
    }

    // ── Occurrence lowering ───────────────────────────────────────────────────

    fn lower_occurrence(
        &self,
        occ:        Option<&AstOcc>,
        _rule_name: &str,
        meta:       &PragmaMeta,
    ) -> Occurrence {
        let cap = |default: usize| -> Capacity {
            match meta.capacity {
                Some(n) => Capacity::Bounded(n),
                None    => {
                    // Warn about default capacity usage
                    Capacity::Bounded(default)
                }
            }
        };

        match occ {
            None                    => Occurrence::Required,
            Some(AstOcc::Optional)  => Occurrence::Optional,
            Some(AstOcc::ZeroOrMore) => Occurrence::ZeroOrMore {
                capacity: cap(self.default_capacity),
            },
            Some(AstOcc::OneOrMore) => Occurrence::OneOrMore {
                capacity: cap(self.default_capacity),
            },
            Some(AstOcc::Bounded { min, max }) => {
                let mn = min.unwrap_or(0) as u32;
                let mx = max.map(|n| n as u32);
                // If min == max we can use a fixed capacity
                let capacity = if let (Some(mn2), Some(mx2)) = (min, max) {
                    if mn2 == mx2 {
                        Capacity::Fixed(*mx2 as usize)
                    } else {
                        cap(mx.unwrap_or(self.default_capacity as u32) as usize)
                    }
                } else {
                    cap(self.default_capacity)
                };
                Occurrence::Bounded { min: mn, max: mx, capacity }
            }
        }
    }
}

/// `a .plus b` (numbers) or `a .cat b` (text or bytes) as one literal.
/// `text` without the leading whitespace its non-blank lines share (RFC 9165
/// `.det`).
fn dedent(text: &str) -> String {
    let indent = text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    text.split_inclusive('\n')
        .map(|l| if l.trim().is_empty() { l.trim_start_matches([' ', '\t']) } else { &l[indent.min(l.len())..] })
        .collect()
}

fn fold_literal(op: &str, a: &Value, b: &Value) -> Option<LiteralValue> {
    let (a, b) = (lower_value(a), lower_value(b));
    Some(match (op, a, b) {
        ("plus", LiteralValue::Uint(x), LiteralValue::Uint(y)) => LiteralValue::Uint(x.checked_add(y)?),
        ("plus", x @ (LiteralValue::Uint(_) | LiteralValue::Int(_)), y @ (LiteralValue::Uint(_) | LiteralValue::Int(_))) => {
            let as_i = |v: LiteralValue| match v { LiteralValue::Uint(n) => n as i128, LiteralValue::Int(n) => n as i128, _ => 0 };
            let sum = as_i(x) + as_i(y);
            if sum >= 0 { LiteralValue::Uint(u64::try_from(sum).ok()?) } else { LiteralValue::Int(i64::try_from(sum).ok()?) }
        }
        ("plus", LiteralValue::Float(x), LiteralValue::Float(y)) => LiteralValue::Float(x + y),
        ("cat", LiteralValue::Text(x), LiteralValue::Text(y)) => LiteralValue::Text(x + &y),
        ("cat", LiteralValue::Bytes(mut x), LiteralValue::Bytes(y)) => { x.extend(y); LiteralValue::Bytes(x) }
        _ => return None,
    })
}

/// The most alternatives group choices can make one map or array (the product
/// of the choices of the groups it includes), a guard against an exponential
/// number of types. WebDriver BiDi's `Command` has 33 (dgtlrift/cddlc#120).
const MAX_ALTERNATIVES: usize = 256;

/// The name of each alternative of a choice of maps, when they all have a member
/// under one key whose value is a text literal, a different one in each
/// (`method: "browser.close"` / `method: "session.end"`): that literal, made a
/// CDDL name (`browser.close`). `None` when there is no such key.
fn discriminants(choices: &[GroupChoice]) -> Option<Vec<String>> {
    use cddlc_parser::MemberKey;
    fn key_of(k: &MemberKey) -> Option<&str> {
        match k {
            MemberKey::Bareword(id) => Some(id.0.as_str()),
            MemberKey::Value(Value::Text(t)) => Some(t.as_str()),
            _ => None,
        }
    }
    // The text literal under `key` in `gc`.
    fn literal<'a>(gc: &'a GroupChoice, key: &str) -> Option<&'a str> {
        gc.entries.iter().find_map(|e| match &e.node.kind {
            MemberKind::KeyValue { key: k, value, .. }
                if e.node.occurrence.is_none() && key_of(&k.node) == Some(key) =>
            {
                match value.node.choices.as_slice() {
                    [t1] if t1.node.ops.is_empty() => match &t1.node.base.node {
                        Type2::Value(Value::Text(t)) => Some(t.as_str()),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        })
    }
    let first = choices.first()?;
    for e in &first.entries {
        let MemberKind::KeyValue { key, .. } = &e.node.kind else { continue };
        let Some(key) = key_of(&key.node) else { continue };
        let Some(names) = choices.iter()
            .map(|gc| literal(gc, key).map(|t| {
                let n: String = t.chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '-' })
                    .collect();
                n.trim_matches(['-', '.']).to_owned()
            }))
            .collect::<Option<Vec<String>>>()
        else { continue };
        // Distinct, non-empty, and still distinct as identifiers.
        let mut seen = std::collections::HashSet::new();
        if names.iter().all(|n| {
            n.starts_with(|c: char| c.is_ascii_alphabetic())
                && seen.insert(n.replace(['.', '-', '_'], "").to_ascii_lowercase())
        }) {
            return Some(names);
        }
    }
    None
}

/// The one Type2 of a type with no choices or operators.
fn single_type2(ty: &Type) -> Option<&Type2> {
    match ty.choices.as_slice() {
        [t1] if t1.node.ops.is_empty() => Some(&t1.node.base.node),
        _ => None,
    }
}

/// The type inside parentheses, `( type )`: a group of one entry that is a
/// bare type (no key, no occurrence).
fn paren_type(ty: &Type) -> Option<&Type> {
    let Some(Type2::Group(g)) = single_type2(ty) else { return None };
    let [gc] = g.node.choices.as_slice() else { return None };
    let [e] = gc.entries.as_slice() else { return None };
    if e.node.occurrence.is_some() { return None }
    match &e.node.kind {
        MemberKind::Type(t) => Some(&t.node),
        _ => None,
    }
}

/// A group without its redundant parentheses: `( a: uint )` parses as a group
/// whose one entry is the parenthesized group.
fn strip_parens(g: &cddlc_parser::GroupEntry) -> &cddlc_parser::GroupEntry {
    if let [gc] = g.choices.as_slice() {
        if let [e] = gc.entries.as_slice() {
            if e.node.occurrence.is_none() {
                if let MemberKind::Type(t) = &e.node.kind {
                    if let Some(Type2::Group(inner)) = single_type2(&t.node) {
                        return strip_parens(&inner.node);
                    }
                }
            }
        }
    }
    g
}

// ── Topological sort ──────────────────────────────────────────────────────────

fn topo_sort(types: &IndexMap<SmolStr, TypeDef>) -> Result<Vec<SmolStr>, IrError> {
    let mut order:   Vec<SmolStr>              = Vec::new();
    let mut visited: HashSet<SmolStr>          = HashSet::new();
    let mut in_stack: HashSet<SmolStr>         = HashSet::new();

    for name in types.keys() {
        if !visited.contains(name.as_str()) {
            topo_visit(name, types, &mut order, &mut visited, &mut in_stack)?;
        }
    }
    Ok(order)
}

fn topo_visit(
    name:     &SmolStr,
    types:    &IndexMap<SmolStr, TypeDef>,
    order:    &mut Vec<SmolStr>,
    visited:  &mut HashSet<SmolStr>,
    in_stack: &mut HashSet<SmolStr>,
) -> Result<(), IrError> {
    if in_stack.contains(name.as_str()) {
        // Cycle detected — not a hard error for CBOR (mutual refs are ok in
        // practice for some targets), just emit in current order.
        return Ok(());
    }
    if visited.contains(name.as_str()) {
        return Ok(());
    }

    in_stack.insert(name.clone());

    if let Some(def) = types.get(name.as_str()) {
        for dep in collect_deps(def) {
            if types.contains_key(dep.as_str()) && !visited.contains(dep.as_str()) {
                topo_visit(&dep, types, order, visited, in_stack)?;
            }
        }
    }

    in_stack.remove(name.as_str());
    visited.insert(name.clone());
    order.push(name.clone());
    Ok(())
}

fn collect_deps(def: &TypeDef) -> Vec<SmolStr> {
    let mut deps = Vec::new();
    match def {
        TypeDef::Struct(s) => {
            for f in &s.fields {
                collect_typeref_deps(&f.ty, &mut deps);
            }
            if let Some(r) = &s.rest {
                collect_typeref_deps(&r.key_ty, &mut deps);
                collect_typeref_deps(&r.val_ty, &mut deps);
            }
        }
        TypeDef::Enum(e) => {
            for v in &e.variants {
                collect_typeref_deps(&v.ty, &mut deps);
            }
        }
        TypeDef::Array(a)  => collect_typeref_deps(&a.element, &mut deps),
        TypeDef::Map(m)    => {
            collect_typeref_deps(&m.key_ty, &mut deps);
            collect_typeref_deps(&m.val_ty, &mut deps);
        }
        TypeDef::Alias(a)  => collect_typeref_deps(&a.ty, &mut deps),
    }
    deps
}

fn collect_typeref_deps(tr: &TypeRef, out: &mut Vec<SmolStr>) {
    match tr {
        TypeRef::Named(n)         => out.push(n.clone()),
        TypeRef::Choice(choices)  => choices.iter().for_each(|c| collect_typeref_deps(c, out)),
        TypeRef::Tagged(_, inner) => collect_typeref_deps(inner, out),
        TypeRef::Primitive(_)     => {}
    }
}

// ── Pragma metadata extraction ────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct PragmaMeta {
    capacity:     Option<usize>,
    str_capacity: Option<usize>,
    regex_hook:   Option<String>,
    doc:          Option<String>,
    deprecated:   bool,
    tag:          Option<CborTag>,
}

impl PragmaMeta {
    fn from_pragmas(pragmas: &[Pragma]) -> Self {
        let mut m = Self::default();
        for p in pragmas {
            match &p.value {
                PragmaValue::Capacity(n)    => m.capacity     = Some(*n as usize),
                PragmaValue::StrCapacity(n) => m.str_capacity = Some(*n as usize),
                PragmaValue::RegexHook(s)   => m.regex_hook   = Some(s.clone()),
                PragmaValue::Doc(s)         => m.doc          = Some(s.clone()),
                PragmaValue::Deprecated     => m.deprecated   = true,
                _ => {}
            }
        }
        m
    }

    fn from_pragmas_with_capacity(
        pragmas:      &[Pragma],
        cap:          Option<usize>,
        str_cap:      Option<usize>,
    ) -> Self {
        let mut m = Self::from_pragmas(pragmas);
        if m.capacity.is_none()     { m.capacity     = cap; }
        if m.str_capacity.is_none() { m.str_capacity = str_cap; }
        m
    }
}

// ── Utility helpers ───────────────────────────────────────────────────────────

/// A variant name for a text literal, usable as an identifier in every
/// backend: its ASCII words in PascalCase (`"free-space"` → `FreeSpace`,
/// `"text/plain"` → `TextPlain`), `Minus` for a leading `-` (`"-0"` →
/// `Minus0`, dgtlrift/cddlc#141), `V` before a leading digit and `Empty`
/// for no words at all.
fn literal_variant_name(s: &str) -> String {
    let mut out = String::new();
    if s.starts_with('-') {
        out.push_str("Minus");
    }
    for word in s.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(f) = chars.next() {
            out.push(f.to_ascii_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() {
        out.push_str("Empty");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'V');
    }
    out
}

fn pascal_case(s: &str) -> String {
    s.split(|c: char| c == '-' || c == '_' || c == ' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None    => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// The type `#mt` or `#mt.ai` stands for (RFC 8610 §3.6), and its constraints.
/// For major types 0 and 1 an additional information below 24 is the value
/// itself; for 7 it picks a simple value or float width. Arrays, maps, bare
/// tags and other simple values aren't modelled, and are `any`.
fn major_type(mt: u8, ai: Option<u64>) -> (TypeRef, Vec<Constraint>) {
    let p = TypeRef::Primitive;
    match (mt, ai) {
        (0, Some(n)) if n < 24 => (p(Primitive::Uint), vec![Constraint::Eq(LiteralValue::Uint(n))]),
        (0, _)                 => (p(Primitive::Uint), vec![]),
        (1, Some(n)) if n < 24 => (p(Primitive::Int), vec![Constraint::Eq(LiteralValue::Int(-1 - n as i64))]),
        (1, _)                 => (p(Primitive::Int), vec![Constraint::ValueRangeInt { min: None, max: Some(-1), inclusive: true }]),
        (2, _)                 => (p(Primitive::Bstr), vec![]),
        (3, _)                 => (p(Primitive::Tstr), vec![]),
        (7, Some(20))          => (p(Primitive::Bool), vec![Constraint::Eq(LiteralValue::Bool(false))]),
        (7, Some(21))          => (p(Primitive::Bool), vec![Constraint::Eq(LiteralValue::Bool(true))]),
        (7, Some(22))          => (p(Primitive::Null), vec![]),
        (7, Some(23))          => (p(Primitive::Undefined), vec![]),
        (7, Some(25))          => (p(Primitive::Float16), vec![]),
        (7, Some(26))          => (p(Primitive::Float32), vec![]),
        (7, Some(27))          => (p(Primitive::Float64), vec![]),
        _                      => (p(Primitive::Any), vec![]),
    }
}

fn literal_to_typeref(lit: &LiteralValue) -> TypeRef {
    match lit {
        LiteralValue::Bool(_)  => TypeRef::Primitive(Primitive::Bool),
        LiteralValue::Null     => TypeRef::Primitive(Primitive::Null),
        LiteralValue::Uint(_)  => TypeRef::Primitive(Primitive::Uint),
        LiteralValue::Int(_)   => TypeRef::Primitive(Primitive::Int),
        LiteralValue::Float(_) => TypeRef::Primitive(Primitive::Float),
        LiteralValue::Bytes(_) => TypeRef::Primitive(Primitive::Bstr),
        LiteralValue::Text(_)  => TypeRef::Primitive(Primitive::Tstr),
    }
}

fn primitive_to_prelude_name(p: &Primitive) -> &'static str {
    match p {
        Primitive::Bool      => "bool",
        Primitive::Null      => "null",
        Primitive::Undefined => "undefined",
        Primitive::Uint      => "uint",
        Primitive::Int       => "int",
        Primitive::Float16   => "float16",
        Primitive::Float32   => "float32",
        Primitive::Float64   => "float64",
        Primitive::Float     => "float",
        Primitive::Bstr      => "bstr",
        Primitive::Tstr      => "tstr",
        Primitive::Any       => "any",
    }
}

fn type_to_name(ty: &Type) -> String {
    if ty.choices.len() == 1 {
        match &ty.choices[0].node.base.node {
            Type2::Typename { name, .. } => name.node.as_str().replace('-', "_"),
            Type2::Value(Value::Uint(n)) => format!("u{n}"),
            Type2::Value(Value::Int(n))  => format!("i{n}"),
            _                            => "anon".into(),
        }
    } else {
        "choice".into()
    }
}

/// The value of a bare `true` or `false` prelude type name, which (RFC 8610
/// Appendix D) admit only that one value.
fn bool_value_name(t2: &Type2) -> Option<bool> {
    match t2 {
        Type2::Typename { name, args } if args.is_empty() => match name.node.as_str() {
            "true"  => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}
