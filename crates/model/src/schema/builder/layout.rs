//! [`SchemaSetBuilder::finish`]: the checks, the named-first renumbering,
//! the effective trait tables and the lookups.
use super::{
    BuildError, ColdEntries, Draft, DraftChildren, DraftId, DraftRole, DraftTraits,
    SchemaSetBuilder,
};
use crate::schema::known::KnownTraits;
use crate::schema::operation::OperationEntry;
use crate::schema::strings::Interner;
use crate::schema::types::{PRELUDE, UNIT};
use crate::schema::uninterpreted::Site;
use crate::schema::{Children, Role, SchemaData, SchemaId, SchemaSet};
use serde_json::Value as Json;

/// A member table has a name-sorted lookup from this many members on.
const LOOKUP_FROM: usize = 16;

/// The trait tables while the set is laid out. Index 0 of each is the
/// empty set, which every schema without traits shares.
struct Tables {
    known: Vec<KnownTraits>,
    cold: Vec<crate::schema::TraitMap>,
    nodes: Vec<Json>,
}

impl Tables {
    fn known(&mut self, known: &KnownTraits, strings: &Interner) -> Result<u32, BuildError> {
        if known.strings().any(|id| !strings.contains(id)) {
            return Err(BuildError::Foreign);
        }
        if *known == KnownTraits::default() {
            return Ok(0);
        }
        self.known.push(known.clone());
        index(self.known.len() - 1)
    }

    fn cold(&mut self, entries: &ColdEntries, strings: &mut Interner) -> Result<u32, BuildError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let mut map = Vec::with_capacity(entries.len());
        for (id, value) in entries {
            let key = strings.intern(id).ok_or(BuildError::TooLarge)?;
            self.nodes.push(value.clone());
            map.push((key, index(self.nodes.len() - 1)?));
        }
        self.cold.push(map.into());
        index(self.cold.len() - 1)
    }
}

fn index(value: usize) -> Result<u32, BuildError> {
    u32::try_from(value).map_err(|_| BuildError::TooLarge)
}

/// Every reference resolves and every list and map is complete.
fn check_complete(builder: &SchemaSetBuilder) -> Result<(), BuildError> {
    for draft in &builder.drafts {
        let fqn = || builder.strings.get(draft.id).into();
        if draft.role == DraftRole::Placeholder {
            return Err(BuildError::NoShapeFound(fqn()));
        }
        if matches!(
            draft.children,
            DraftChildren::List(None) | DraftChildren::Map(None)
        ) && draft.role == DraftRole::Named
        {
            return Err(BuildError::InvalidShape(fqn()));
        }
    }
    Ok(())
}

/// The drafts in set order: named, their members, overlays, the prelude.
fn layout(builder: &SchemaSetBuilder) -> Vec<DraftId> {
    let mut order = builder.order.clone();
    for named in &builder.order {
        match &builder.drafts[named.index()].children {
            DraftChildren::Members(members) => order.extend(members),
            DraftChildren::List(member) => order.extend(member),
            DraftChildren::Map(entry) => order.extend(entry.iter().flat_map(|(k, v)| [*k, *v])),
            DraftChildren::None => {}
        }
    }
    order.extend(&builder.overlays);
    let prelude = builder.drafts.iter().enumerate();
    let prelude = prelude.filter(|(_, draft)| draft.role == DraftRole::Prelude);
    order.extend(prelude.map(|(index, _)| DraftId(index as u32)));
    order
}

pub(super) fn finish(mut builder: SchemaSetBuilder) -> Result<SchemaSet, BuildError> {
    check_complete(&builder)?;
    let order = layout(&builder);
    let mut final_id = vec![SchemaId(u32::MAX); builder.drafts.len()];
    for (position, draft) in order.iter().enumerate() {
        final_id[draft.index()] = SchemaId(index(position)?);
    }
    let remap = |id: DraftId| final_id[id.index()];
    let (members, member_lookup, children) = member_tables(&builder, &order, &remap)?;
    let schemas = schema_data(&mut builder, &order, &remap, &children)?;
    let named = index(builder.order.len())?;
    let by_name = by_name(&builder, &schemas.0, named)?;
    let operations = operations(&mut builder, &remap)?;
    check_sites(&builder, &schemas.0, &by_name)?;
    let (schemas, tables) = schemas;
    Ok(SchemaSet {
        strings: builder.strings.finish(),
        schemas: schemas.into(),
        named,
        members: members.into(),
        member_lookup: member_lookup.into(),
        known: tables.known.into(),
        cold: tables.cold.into(),
        nodes: tables.nodes.into(),
        by_name: by_name.into(),
        operations: operations.into(),
        service: builder.service,
        uninterpreted: builder.uninterpreted.into(),
    })
}

type MemberTables = (Vec<SchemaId>, Vec<SchemaId>, Vec<Children>);

/// The member table, the name-sorted lookups and each named or prelude
/// draft's children, by draft index.
fn member_tables(
    builder: &SchemaSetBuilder,
    order: &[DraftId],
    remap: &impl Fn(DraftId) -> SchemaId,
) -> Result<MemberTables, BuildError> {
    let mut members = Vec::new();
    let mut lookup = Vec::new();
    let mut children = vec![Children::None; builder.drafts.len()];
    for draft_id in order {
        let draft = &builder.drafts[draft_id.index()];
        if !matches!(draft.role, DraftRole::Named | DraftRole::Prelude) {
            continue;
        }
        children[draft_id.index()] = match &draft.children {
            DraftChildren::Members(list) => {
                let start = index(members.len())?;
                members.extend(list.iter().map(|id| remap(*id)));
                let sorted = (list.len() >= LOOKUP_FROM)
                    .then(|| sorted_members(builder, list, remap, &mut lookup))
                    .transpose()?;
                Children::Members {
                    start,
                    len: index(list.len())?,
                    lookup: sorted,
                }
            }
            DraftChildren::List(Some(member)) => Children::List(remap(*member)),
            DraftChildren::Map(Some((key, value))) => Children::Map(remap(*key), remap(*value)),
            DraftChildren::List(None) | DraftChildren::Map(None) | DraftChildren::None => {
                Children::None
            }
        };
    }
    Ok((members, lookup, children))
}

fn member_name(builder: &SchemaSetBuilder, id: DraftId) -> &str {
    match builder.drafts[id.index()].role {
        DraftRole::Member { name, .. } => builder.strings.get(name),
        _ => "",
    }
}

fn sorted_members(
    builder: &SchemaSetBuilder,
    list: &[DraftId],
    remap: &impl Fn(DraftId) -> SchemaId,
    lookup: &mut Vec<SchemaId>,
) -> Result<u32, BuildError> {
    let start = index(lookup.len())?;
    let mut sorted = list.to_vec();
    sorted.sort_by(|a, b| member_name(builder, *a).cmp(member_name(builder, *b)));
    lookup.extend(sorted.into_iter().map(remap));
    Ok(start)
}

/// The draft a member or overlay reads its type and children from.
const fn source(draft: &Draft, id: DraftId) -> DraftId {
    match draft.role {
        DraftRole::Member { target, .. } | DraftRole::Overlay { target } => target,
        _ => id,
    }
}

/// Each schema's data in set order, with the effective trait tables.
fn schema_data(
    builder: &mut SchemaSetBuilder,
    order: &[DraftId],
    remap: &impl Fn(DraftId) -> SchemaId,
    children: &[Children],
) -> Result<(Vec<SchemaData>, Tables), BuildError> {
    let mut tables = Tables {
        known: vec![KnownTraits::default()],
        cold: vec![Box::default()],
        nodes: Vec::new(),
    };
    // (known, cold) of each draft once laid out; targets come first.
    let mut sets = vec![(0, 0); builder.drafts.len()];
    let mut schemas = Vec::with_capacity(order.len());
    for &id in order {
        let draft = &builder.drafts[id.index()];
        let from = source(draft, id);
        let target = &builder.drafts[from.index()];
        let (known, cold, own_traits) = match &draft.traits {
            DraftTraits::Inherit => (sets[from.index()].0, sets[from.index()].1, false),
            DraftTraits::Own(known, cold) if from != id => {
                let same = matches!(&target.traits,
                    DraftTraits::Own(k, c) if k == known && same_cold(c, cold));
                if same {
                    (sets[from.index()].0, sets[from.index()].1, false)
                } else {
                    let known = tables.known(known, &builder.strings)?;
                    (known, tables.cold(cold, &mut builder.strings)?, true)
                }
            }
            DraftTraits::Own(known, cold) => {
                let known = tables.known(known, &builder.strings)?;
                (known, tables.cold(cold, &mut builder.strings)?, false)
            }
        };
        sets[id.index()] = (known, cold);
        let role = match draft.role {
            DraftRole::Member {
                container,
                name,
                target,
            } => Role::Member {
                name,
                index: member_index(builder, container, id)?,
                target: remap(target),
            },
            DraftRole::Overlay { .. } => Role::Overlay,
            DraftRole::Prelude => Role::Prelude,
            DraftRole::Named | DraftRole::Placeholder => Role::Named,
        };
        schemas.push(SchemaData {
            id: draft.id,
            sharp_at: draft.sharp_at,
            dollar_at: draft.dollar_at,
            shape_type: target.shape_type,
            type_name: target.type_name.ok_or(BuildError::Foreign)?,
            role,
            children: children[from.index()],
            known,
            cold,
            own_traits,
        });
    }
    Ok((schemas, tables))
}

fn same_cold(a: &[(Box<str>, Json)], b: &[(Box<str>, Json)]) -> bool {
    let get = |list: &'_ [(Box<str>, Json)], id: &str| {
        list.iter()
            .find(|(key, _)| &**key == id)
            .map(|(_, v)| v.clone())
    };
    a.len() == b.len()
        && a.iter()
            .all(|(id, value)| get(b, id).as_ref() == Some(value))
}

fn member_index(
    builder: &SchemaSetBuilder,
    container: DraftId,
    member: DraftId,
) -> Result<u32, BuildError> {
    let position = match &builder.drafts[container.index()].children {
        DraftChildren::Members(list) => list.iter().position(|id| *id == member),
        DraftChildren::Map(Some((_, value))) if *value == member => Some(1),
        _ => Some(0),
    };
    index(position.unwrap_or(0))
}

/// The named range sorted by short name; two shapes with one name clash.
fn by_name(
    builder: &SchemaSetBuilder,
    schemas: &[SchemaData],
    named: u32,
) -> Result<Vec<SchemaId>, BuildError> {
    let name = |id: &SchemaId| {
        let data = &schemas[id.index()];
        let fqn = builder.strings.get(data.id);
        let end = data.dollar_at.map_or(fqn.len(), usize::from);
        &fqn[usize::from(data.sharp_at) + 1..end]
    };
    let mut sorted: Vec<SchemaId> = (0..named).map(SchemaId).collect();
    sorted.sort_by(|a, b| name(a).cmp(name(b)));
    if let Some(pair) = sorted
        .windows(2)
        .find(|pair| name(&pair[0]) == name(&pair[1]))
    {
        return Err(BuildError::NameClash(name(&pair[0]).into()));
    }
    Ok(sorted)
}

/// The operations with their roots renumbered; `Unit` roots are none.
fn operations(
    builder: &mut SchemaSetBuilder,
    remap: &impl Fn(DraftId) -> SchemaId,
) -> Result<Vec<OperationEntry>, BuildError> {
    let unit = builder
        .prelude(PRELUDE[UNIT].0)
        .ok_or(BuildError::Foreign)?;
    let root = |id: Option<DraftId>| id.filter(|id| *id != unit).map(remap);
    let entries = std::mem::take(&mut builder.operations);
    let entries = entries
        .into_iter()
        .map(|(data, id, sharp_at, shapes)| OperationEntry {
            data,
            id,
            sharp_at,
            input: root(shapes.input),
            output: root(shapes.output),
            errors: shapes.errors.into_iter().map(remap).collect(),
        });
    Ok(entries.collect())
}

/// Every uninterpreted key's operation or shape exists, so none is
/// recorded where no call can reach it.
fn check_sites(
    builder: &SchemaSetBuilder,
    schemas: &[SchemaData],
    by_name: &[SchemaId],
) -> Result<(), BuildError> {
    let short = |id: &SchemaId| {
        let data = &schemas[id.index()];
        &builder.strings.get(data.id)[usize::from(data.sharp_at) + 1..]
    };
    let shape_exists = |name: &str| by_name.binary_search_by(|id| short(id).cmp(name)).is_ok();
    for key in &builder.uninterpreted {
        let known = match &key.site {
            Site::Service => true,
            Site::Operation(name) => builder.operation_names.contains(name),
            Site::Shape(name) => shape_exists(name),
        };
        if !known {
            return Err(BuildError::UnknownSite(key.to_string().into()));
        }
    }
    Ok(())
}
