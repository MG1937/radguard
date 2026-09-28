use std::collections::{hash_map::Entry, HashMap, HashSet, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};

use crate::graph::{Graph, NodeId};
use crate::ir::{Statement, VariableId};

pub type Location = usize;

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

#[derive(Default)]
pub struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517cc1b727220a95);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn write_i8(&mut self, value: i8) {
        self.write_u64(value as u64);
    }

    fn write_i16(&mut self, value: i16) {
        self.write_u64(value as u64);
    }

    fn write_i32(&mut self, value: i32) {
        self.write_u64(value as u64);
    }

    fn write_i64(&mut self, value: i64) {
        self.write_u64(value as u64);
    }

    fn write_isize(&mut self, value: isize) {
        self.write_u64(value as u64);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Definition {
    Parameter(VariableId),
    Instruction(Location),
}

#[derive(Debug, Default)]
pub struct DefUseChains {
    pub use_def: FastHashMap<(VariableId, Location), Vec<Definition>>,
    pub def_use: FastHashMap<(VariableId, Definition), Vec<Location>>,
}

impl DefUseChains {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            use_def: FastHashMap::with_capacity_and_hasher(capacity, Default::default()),
            def_use: FastHashMap::with_capacity_and_hasher(capacity, Default::default()),
        }
    }
}

#[derive(Clone, Copy)]
struct StatementLocation {
    node: NodeId,
    index: usize,
}

struct LocationIndex {
    statements: Vec<StatementLocation>,
    by_node: Vec<Vec<Location>>,
    has_instruction_definitions: bool,
}

struct ActiveLocations {
    by_node: Vec<Vec<Location>>,
    active: Vec<bool>,
    count: usize,
}

impl LocationIndex {
    fn build(graph: &Graph) -> Self {
        let mut statements = Vec::new();
        let mut by_node = vec![Vec::new(); graph.nodes.len()];
        let mut has_instruction_definitions = false;
        for &node in &graph.rpo {
            for index in 0..graph.nodes[node].statements.len() {
                by_node[node].push(statements.len());
                statements.push(StatementLocation { node, index });
                has_instruction_definitions |= graph.nodes[node].statements[index]
                    .defined_variable()
                    .is_some();
            }
        }
        Self {
            statements,
            by_node,
            has_instruction_definitions,
        }
    }

    fn build_with_definition_sites(graph: &Graph) -> (Self, Vec<(Location, usize)>) {
        let mut statements = Vec::new();
        let mut by_node = vec![Vec::new(); graph.nodes.len()];
        let mut definition_sites = Vec::new();
        for &node in &graph.rpo {
            for index in 0..graph.nodes[node].statements.len() {
                let location = statements.len();
                by_node[node].push(location);
                statements.push(StatementLocation { node, index });
                if let Some(variable) = graph.nodes[node].statements[index].defined_variable() {
                    let variable_index = graph
                        .variables
                        .dense_index(variable)
                        .expect("defined variable is registered");
                    definition_sites.push((location, variable_index));
                }
            }
        }
        (
            Self {
                statements,
                by_node,
                has_instruction_definitions: !definition_sites.is_empty(),
            },
            definition_sites,
        )
    }

    fn get(&self, location: Location) -> StatementLocation {
        self.statements[location]
    }

    fn node(&self, location: Location) -> NodeId {
        self.get(location).node
    }

    fn node_locations(&self, node: NodeId) -> &[Location] {
        &self.by_node[node]
    }
}

impl ActiveLocations {
    fn new(locations: &LocationIndex) -> Self {
        Self {
            by_node: locations.by_node.clone(),
            active: vec![true; locations.statements.len()],
            count: locations.statements.len(),
        }
    }

    fn total_count(&self) -> usize {
        self.count
    }

    fn count(&self, node: NodeId) -> usize {
        self.by_node[node].len()
    }

    fn is_removed(&self, location: Location) -> bool {
        !self.active[location]
    }

    fn select(&self, _locations: &LocationIndex, node: NodeId, rank: usize) -> Location {
        self.by_node[node][rank]
    }

    fn remove(&mut self, locations: &LocationIndex, location: Location) {
        if self.is_removed(location) {
            return;
        }
        self.active[location] = false;
        self.count -= 1;
        let statement = locations.get(location);
        let index = self.by_node[statement.node]
            .binary_search(&location)
            .expect("active location is present in its node");
        self.by_node[statement.node].remove(index);
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ReachingDefinitions {
    bits: Vec<u64>,
}

struct DefinitionUniverse {
    definitions: Vec<Definition>,
    offsets: Vec<usize>,
    location_bits: Vec<usize>,
    blocked_words: Vec<Vec<(usize, u64)>>,
    generated_words: Vec<Vec<(usize, u64)>>,
}

impl DefinitionUniverse {
    fn build(
        graph: &Graph,
        locations: &LocationIndex,
        definition_sites: &[(Location, usize)],
    ) -> Self {
        let variable_count = graph.variables.dense_len();
        let parameters = graph.variables.parameter_ids();
        let mut counts = vec![0_usize; variable_count];
        for &parameter in &parameters {
            let index = graph.variables.dense_index(parameter).expect("known parameter");
            counts[index] += 1;
        }
        for &(_, index) in definition_sites {
            counts[index] += 1;
        }

        let mut offsets = Vec::with_capacity(variable_count + 1);
        offsets.push(0);
        for &count in &counts {
            offsets.push(offsets.last().copied().unwrap_or(0) + count);
        }
        let total = offsets.last().copied().unwrap_or(0);
        let mut definitions = vec![Definition::Instruction(usize::MAX); total];
        for (cursor, &offset) in counts.iter_mut().zip(&offsets) {
            *cursor = offset;
        }
        for parameter in parameters {
            let index = graph.variables.dense_index(parameter).expect("known parameter");
            definitions[counts[index]] = Definition::Parameter(parameter);
            counts[index] += 1;
        }

        let mut location_bits = vec![usize::MAX; locations.statements.len()];
        let mut blocked_words = vec![Vec::new(); graph.nodes.len()];
        let mut generated_words = vec![Vec::new(); graph.nodes.len()];
        let mut last_definition = vec![usize::MAX; variable_count];
        let mut node_variables = Vec::new();
        let mut site_cursor = 0;
        for &node in &graph.rpo {
            node_variables.clear();
            let node_locations = locations.node_locations(node);
            let node_end = node_locations.last().map_or(0, |location| location + 1);
            while definition_sites
                .get(site_cursor)
                .is_some_and(|(location, _)| *location < node_end)
            {
                let (location, index) = definition_sites[site_cursor];
                site_cursor += 1;
                let bit = counts[index];
                counts[index] += 1;
                definitions[bit] = Definition::Instruction(location);
                location_bits[location] = bit;
                if last_definition[index] == usize::MAX {
                    node_variables.push(index);
                }
                last_definition[index] = bit;
            }
            for &index in &node_variables {
                append_word_range_mask(
                    &mut blocked_words[node],
                    offsets[index]..offsets[index + 1],
                );
                let bit = last_definition[index];
                generated_words[node].push((bit / 64, 1_u64 << (bit % 64)));
                last_definition[index] = usize::MAX;
            }
            merge_word_masks(&mut blocked_words[node]);
            merge_word_masks(&mut generated_words[node]);
        }
        Self {
            definitions,
            offsets,
            location_bits,
            blocked_words,
            generated_words,
        }
    }

    fn range(&self, graph: &Graph, variable: VariableId) -> std::ops::Range<usize> {
        let index = graph.variables.dense_index(variable).expect("known variable");
        self.offsets[index]..self.offsets[index + 1]
    }

    fn bit(&self, graph: &Graph, variable: VariableId, definition: Definition) -> usize {
        match definition {
            Definition::Parameter(_) => self.range(graph, variable).start,
            Definition::Instruction(location) => self.location_bits[location],
        }
    }
}

fn append_word_range_mask(words: &mut Vec<(usize, u64)>, range: std::ops::Range<usize>) {
    if range.is_empty() {
        return;
    }
    let first_word = range.start / 64;
    let last_word = (range.end - 1) / 64;
    let first_mask = u64::MAX << (range.start % 64);
    let last_mask = u64::MAX >> (63 - ((range.end - 1) % 64));
    if first_word == last_word {
        words.push((first_word, first_mask & last_mask));
        return;
    }
    words.push((first_word, first_mask));
    for word in first_word + 1..last_word {
        words.push((word, u64::MAX));
    }
    words.push((last_word, last_mask));
}

fn merge_word_masks(words: &mut Vec<(usize, u64)>) {
    words.sort_unstable_by_key(|(word, _)| *word);
    let mut output = 0;
    for input in 0..words.len() {
        if output != 0 && words[output - 1].0 == words[input].0 {
            words[output - 1].1 |= words[input].1;
        } else {
            words[output] = words[input];
            output += 1;
        }
    }
    words.truncate(output);
}

fn clear_bit_range(bits: &mut [u64], range: std::ops::Range<usize>) {
    if range.is_empty() {
        return;
    }
    let first_word = range.start / 64;
    let last_word = (range.end - 1) / 64;
    let first_mask = u64::MAX << (range.start % 64);
    let last_mask = u64::MAX >> (63 - ((range.end - 1) % 64));
    if first_word == last_word {
        bits[first_word] &= !(first_mask & last_mask);
        return;
    }
    bits[first_word] &= !first_mask;
    bits[last_word] &= !last_mask;
    bits[first_word + 1..last_word].fill(0);
}

impl ReachingDefinitions {
    fn empty(universe: &DefinitionUniverse) -> Self {
        Self {
            bits: vec![0; universe.definitions.len().div_ceil(64)],
        }
    }

    fn definitions(
        &self,
        graph: &Graph,
        universe: &DefinitionUniverse,
        variable: VariableId,
    ) -> Vec<Definition> {
        let range = universe.range(graph, variable);
        let mut definitions = Vec::with_capacity(1);
        for bit in range {
            if self.bits[bit / 64] & (1_u64 << (bit % 64)) != 0 {
                definitions.push(universe.definitions[bit]);
            }
        }
        definitions
    }

    fn insert(
        &mut self,
        graph: &Graph,
        universe: &DefinitionUniverse,
        variable: VariableId,
        definition: Definition,
    ) {
        let bit = universe.bit(graph, variable, definition);
        self.bits[bit / 64] |= 1_u64 << (bit % 64);
    }

    fn replace(
        &mut self,
        graph: &Graph,
        universe: &DefinitionUniverse,
        variable: VariableId,
        definition: Definition,
    ) {
        clear_bit_range(&mut self.bits, universe.range(graph, variable));
        self.insert(graph, universe, variable, definition);
    }

    fn union_transfer_from(
        &mut self,
        other: &Self,
        universe: &DefinitionUniverse,
        node: NodeId,
    ) -> bool {
        let blocked = &universe.blocked_words[node];
        let generated = &universe.generated_words[node];
        let mut blocked_index = 0;
        let mut generated_index = 0;
        let mut changed = false;
        for (word, (current, source)) in self.bits.iter_mut().zip(&other.bits).enumerate() {
            let blocked_mask = if blocked
                .get(blocked_index)
                .is_some_and(|(blocked_word, _)| *blocked_word == word)
            {
                let mask = blocked[blocked_index].1;
                blocked_index += 1;
                mask
            } else {
                0
            };
            let generated_mask = if generated
                .get(generated_index)
                .is_some_and(|(generated_word, _)| *generated_word == word)
            {
                let mask = generated[generated_index].1;
                generated_index += 1;
                mask
            } else {
                0
            };
            let merged = *current | (*source & !blocked_mask) | generated_mask;
            changed |= merged != *current;
            *current = merged;
        }
        changed
    }

    fn union_generated(&mut self, universe: &DefinitionUniverse, node: NodeId) -> bool {
        let mut changed = false;
        for &(word, mask) in &universe.generated_words[node] {
            let merged = self.bits[word] | mask;
            changed |= merged != self.bits[word];
            self.bits[word] = merged;
        }
        changed
    }
}

fn union_predecessor_transfer(
    states: &mut [ReachingDefinitions],
    node: NodeId,
    predecessor: NodeId,
    universe: &DefinitionUniverse,
) -> bool {
    if node == predecessor {
        return states[node].union_generated(universe, predecessor);
    }
    if predecessor < node {
        let (before_node, node_and_after) = states.split_at_mut(node);
        node_and_after[0].union_transfer_from(&before_node[predecessor], universe, predecessor)
    } else {
        let (before_predecessor, predecessor_and_after) = states.split_at_mut(predecessor);
        before_predecessor[node].union_transfer_from(
            &predecessor_and_after[0],
            universe,
            predecessor,
        )
    }
}

pub fn optimize(graph: &mut Graph) -> DefUseChains {
    optimize_impl::<true>(graph)
}

pub(crate) fn optimize_for_decompilation(graph: &mut Graph) {
    drop(optimize_impl::<false>(graph));
}

fn optimize_impl<const PRESERVE_CHAINS: bool>(graph: &mut Graph) -> DefUseChains {
    let (locations, definition_sites) = LocationIndex::build_with_definition_sites(graph);
    let mut chains = build_def_use_at(graph, &locations, &definition_sites);
    if !locations.has_instruction_definitions {
        return chains;
    }
    split_variables_at(graph, &mut chains, &locations);
    let mut live_locations = ActiveLocations::new(&locations);
    dead_code_elimination(graph, &locations, &mut live_locations, &mut chains);
    if chains.use_def.is_empty() && chains.def_use.is_empty() {
        compact_removed_without_index(graph, &locations, &live_locations);
        return chains;
    }
    if !chains_have_instruction_definitions(&chains) {
        remap_chain_locations(&mut chains, &locations, &live_locations);
        compact_removed_without_index(graph, &locations, &live_locations);
        return chains;
    }
    register_propagation(
        graph,
        &locations,
        &mut live_locations,
        &mut chains,
    );
    if PRESERVE_CHAINS {
        remap_chain_locations(&mut chains, &locations, &live_locations);
        let compacted_locations = compact_removed(graph, &locations, &live_locations);
        place_declarations_at(graph, &chains, &compacted_locations, None);
    } else {
        place_declarations_at(graph, &chains, &locations, Some(&live_locations));
        compact_removed_without_index(graph, &locations, &live_locations);
    }
    chains
}

pub fn build_def_use(graph: &Graph) -> DefUseChains {
    let (locations, definition_sites) = LocationIndex::build_with_definition_sites(graph);
    build_def_use_at(graph, &locations, &definition_sites)
}

fn collect_unique_used_variables(statement: &Statement, output: &mut Vec<VariableId>) {
    output.clear();
    statement.collect_used_variables_vec(output);
    if output.len() > 1 {
        output.sort_unstable();
        output.dedup();
    }
}

fn chains_have_instruction_definitions(chains: &DefUseChains) -> bool {
    let in_def_use = chains
        .def_use
        .keys()
        .any(|(_, definition)| matches!(definition, Definition::Instruction(_)));
    debug_assert_eq!(
        in_def_use,
        chains
            .use_def
            .values()
            .flatten()
            .any(|definition| matches!(definition, Definition::Instruction(_)))
    );
    in_def_use
}

fn build_def_use_at(
    graph: &Graph,
    locations: &LocationIndex,
    definition_sites: &[(Location, usize)],
) -> DefUseChains {
    if let Some(chains) = build_linear_def_use(graph, locations) {
        return chains;
    }

    let universe = DefinitionUniverse::build(graph, locations, definition_sites);
    let mut incoming = vec![ReachingDefinitions::empty(&universe); graph.nodes.len()];
    for parameter in graph.variables.parameter_ids() {
        incoming[graph.entry].insert(
            graph,
            &universe,
            parameter,
            Definition::Parameter(parameter),
        );
    }

    let mut queue = VecDeque::from(graph.rpo.clone());
    let mut queued = vec![false; graph.nodes.len()];
    let mut processed = vec![false; graph.nodes.len()];
    for &node in &graph.rpo {
        queued[node] = true;
    }
    while let Some(node) = queue.pop_front() {
        queued[node] = false;
        let first_visit = !processed[node];
        let mut incoming_changed = first_visit;
        for predecessor in graph.all_predecessors(node) {
            incoming_changed |=
                union_predecessor_transfer(&mut incoming, node, predecessor, &universe);
        }
        if !incoming_changed {
            continue;
        }
        processed[node] = true;

        for successor in graph.all_successors(node) {
            if !queued[successor] {
                queued[successor] = true;
                queue.push_back(successor);
            }
        }
    }

    let mut chains = DefUseChains::with_capacity(locations.statements.len());
    let mut used_variables = Vec::new();
    for &node in &graph.rpo {
        let reaching = &mut incoming[node];
        for &location in locations.node_locations(node) {
            let statement_location = locations.get(location);
            let statement = &graph.nodes[node].statements[statement_location.index];
            used_variables.clear();
            statement.collect_unique_used_variables_vec(&mut used_variables);
            for &variable in &used_variables {
                let mut definitions = reaching.definitions(graph, &universe, variable);
                if definitions.is_empty() {
                    continue;
                }
                definitions.sort_by_key(definition_sort_key);
                for &definition in &definitions {
                    chains
                        .def_use
                        .entry((variable, definition))
                        .or_default()
                        .push(location);
                }
                chains
                    .use_def
                    .insert((variable, location), definitions);
            }
            if let Some(variable) = statement.defined_variable() {
                reaching.replace(
                    graph,
                    &universe,
                    variable,
                    Definition::Instruction(location),
                );
            }
        }
    }
    chains
}

fn build_linear_def_use(graph: &Graph, locations: &LocationIndex) -> Option<DefUseChains> {
    let &[node] = graph.rpo.as_slice() else {
        return None;
    };
    if graph.all_predecessors(node).next().is_some()
        || graph.all_successors(node).next().is_some()
    {
        return None;
    }

    let mut chains = DefUseChains::with_capacity(locations.statements.len());
    let mut definitions = FastHashMap::<VariableId, Definition>::default();
    for parameter in graph.variables.parameter_ids() {
        definitions.insert(parameter, Definition::Parameter(parameter));
    }

    let mut used_variables = Vec::new();
    for &location in locations.node_locations(node) {
        let statement_location = locations.get(location);
        let statement = &graph.nodes[statement_location.node].statements[statement_location.index];
        used_variables.clear();
        statement.collect_unique_used_variables_vec(&mut used_variables);
        for &variable in &used_variables {
            let Some(&definition) = definitions.get(&variable) else {
                continue;
            };
            chains
                .def_use
                .entry((variable, definition))
                .or_default()
                .push(location);
            chains
                .use_def
                .insert((variable, location), vec![definition]);
        }
        if let Some(variable) = statement.defined_variable() {
            definitions.insert(variable, Definition::Instruction(location));
        }
    }
    Some(chains)
}

pub fn split_variables(graph: &mut Graph, chains: &mut DefUseChains) {
    let locations = LocationIndex::build(graph);
    split_variables_at(graph, chains, &locations);
}

fn split_variables_at(
    graph: &mut Graph,
    chains: &mut DefUseChains,
    locations: &LocationIndex,
) {
    let mut definitions_by_variable = FastHashMap::<VariableId, Vec<Definition>>::default();
    for &(variable, definition) in chains.def_use.keys() {
        definitions_by_variable
            .entry(variable)
            .or_default()
            .push(definition);
    }

    let mut variables: Vec<_> = definitions_by_variable.into_iter().collect();
    variables.sort_by_key(|(variable, _)| *variable);
    let mut definition_versions = FastHashMap::default();
    let mut use_marks = vec![false; locations.statements.len()];
    for (variable, mut definitions) in variables {
        definitions.sort_by_key(definition_order_key);
        definitions.dedup();
        let ordered_definitions = definitions.clone();
        let mut remaining = vec![true; ordered_definitions.len()];
        let mut groups = Vec::new();
        let mut definition_queue = VecDeque::new();
        while let Some((_, start)) = ordered_definitions
            .iter()
            .copied()
            .enumerate()
            .find(|(index, _)| remaining[*index])
        {
            let mut group_definitions = Vec::new();
            let mut group_uses = Vec::new();
            definition_queue.clear();
            definition_queue.push_back(start);
            while let Some(definition) = definition_queue.pop_front() {
                let definition_index = ordered_definitions
                    .binary_search_by(|candidate| {
                        definition_order_key(candidate).cmp(&definition_order_key(&definition))
                    })
                    .expect("definition is present in ordered definitions");
                if !remaining[definition_index] {
                    continue;
                }
                remaining[definition_index] = false;
                group_definitions.push(definition);
                if let Some(uses) = chains.def_use.get(&(variable, definition)) {
                    for &usage in uses {
                        if !use_marks[usage] {
                            use_marks[usage] = true;
                            group_uses.push(usage);
                            if let Some(reaching) = chains.use_def.get(&(variable, usage)) {
                                definition_queue.extend(reaching.iter().copied());
                            }
                        }
                    }
                }
            }
            for &usage in &group_uses {
                use_marks[usage] = false;
            }
            groups.push((group_definitions, group_uses));
        }
        if groups.len() <= 1 {
            continue;
        }

        let original_metadata = graph
            .variables
            .cloned_metadata(variable)
            .expect("def-use references a known variable");
        for (version, (definitions, uses)) in groups.into_iter().enumerate() {
            let replacement = if definitions.contains(&Definition::Parameter(variable)) {
                graph
                    .variables
                    .set_versioned_name(variable, &original_metadata, version);
                variable
            } else {
                graph
                    .variables
                    .version_from_metadata(variable, version, &original_metadata)
            };
            for &definition in &definitions {
                definition_versions.insert((variable, definition), replacement);
            }
            if replacement != variable {
                for location in uses {
                    let statement = locations.get(location);
                    let mut remap = |candidate| {
                        if candidate == variable {
                            replacement
                        } else {
                            candidate
                        }
                    };
                    graph.nodes[statement.node].statements[statement.index]
                        .remap_used_variables(&mut remap);
                    let definitions = chains
                        .use_def
                        .remove(&(variable, location))
                        .expect("split use has a use-def chain");
                    let previous = chains
                        .use_def
                        .insert((replacement, location), definitions);
                    debug_assert!(previous.is_none());
                }
            }
            for definition in definitions {
                let Definition::Instruction(location) = definition else {
                    continue;
                };
                let statement = locations.get(location);
                graph.nodes[statement.node].statements[statement.index]
                    .replace_defined_variable(replacement);
            }
        }
    }
    remap_definition_chain_variables(chains, &definition_versions);
}

fn remap_definition_chain_variables(
    chains: &mut DefUseChains,
    definition_versions: &FastHashMap<(VariableId, Definition), VariableId>,
) {
    let def_use = std::mem::take(&mut chains.def_use);
    for ((variable, definition), uses) in def_use {
        let variable = definition_versions
            .get(&(variable, definition))
            .copied()
            .unwrap_or(variable);
        chains.def_use.insert((variable, definition), uses);
    }
}

fn remap_chain_locations(
    chains: &mut DefUseChains,
    location_index: &LocationIndex,
    live_locations: &ActiveLocations,
) {
    let mut locations = vec![None; location_index.statements.len()];
    let mut next = 0;
    for (location, remapped) in locations.iter_mut().enumerate() {
        if !live_locations.is_removed(location) {
            *remapped = Some(next);
            next += 1;
        }
    }

    let use_def = std::mem::take(&mut chains.use_def);
    chains.use_def =
        FastHashMap::with_capacity_and_hasher(use_def.len(), Default::default());
    for ((variable, location), definitions) in use_def {
        let Some(location) = locations[location] else {
            continue;
        };
        let definitions: Vec<_> = definitions
            .into_iter()
            .filter_map(|definition| remap_definition_location(definition, &locations))
            .collect();
        if !definitions.is_empty() {
            chains.use_def.insert((variable, location), definitions);
        }
    }

    let def_use = std::mem::take(&mut chains.def_use);
    chains.def_use =
        FastHashMap::with_capacity_and_hasher(def_use.len(), Default::default());
    for ((variable, definition), uses) in def_use {
        let Some(definition) = remap_definition_location(definition, &locations) else {
            continue;
        };
        let uses: Vec<_> = uses
            .into_iter()
            .filter_map(|location| locations[location])
            .collect();
        if !uses.is_empty() {
            chains.def_use.insert((variable, definition), uses);
        }
    }
}

fn remap_definition_location(
    definition: Definition,
    locations: &[Option<Location>],
) -> Option<Definition> {
    match definition {
        Definition::Parameter(variable) => Some(Definition::Parameter(variable)),
        Definition::Instruction(location) => locations[location].map(Definition::Instruction),
    }
}

fn definition_order_key(definition: &Definition) -> (u8, usize) {
    match definition {
        Definition::Parameter(_) => (0, 0),
        Definition::Instruction(location) => (1, *location),
    }
}

fn dead_code_elimination(
    graph: &mut Graph,
    locations: &LocationIndex,
    live_locations: &mut ActiveLocations,
    chains: &mut DefUseChains,
) {
    for rpo_index in 0..graph.rpo.len() {
        let node = graph.rpo[rpo_index];
        let mut cursor = 0;
        while cursor < live_locations.count(node) {
            let location = live_locations.select(locations, node, cursor);
            let statement_location = locations.get(location);
            let statement =
                &graph.nodes[statement_location.node].statements[statement_location.index];
            let Some(variable) = statement.defined_variable() else {
                cursor += 1;
                continue;
            };
            if chains
                .def_use
                .contains_key(&(variable, Definition::Instruction(location)))
            {
                cursor += 1;
                continue;
            }
            if statement.is_call() {
                graph.nodes[statement_location.node].statements[statement_location.index]
                    .remove_defined_variable();
                cursor += 1;
            } else if statement.has_side_effect() {
                cursor += 1;
            } else {
                update_chain(graph, locations, live_locations, chains, location);
                remove_live_location(locations, live_locations, location);
                cursor += 1;
            }
        }
    }
}

fn update_chain(
    graph: &mut Graph,
    locations: &LocationIndex,
    live_locations: &mut ActiveLocations,
    chains: &mut DefUseChains,
    location: Location,
) {
    if live_locations.is_removed(location) {
        return;
    }
    let statement_location = locations.get(location);
    let mut used_variables = Vec::new();
    collect_unique_used_variables(
        &graph.nodes[statement_location.node].statements[statement_location.index],
        &mut used_variables,
    );
    for variable in used_variables {
        let definitions = chains
            .use_def
            .remove(&(variable, location))
            .unwrap_or_default();
        for definition in definitions {
            let key = (variable, definition);
            let direct_removal = chains
                .def_use
                .get(&key)
                .is_some_and(|uses| uses.as_slice() == [location]);
            let became_empty = if direct_removal {
                chains.def_use.remove(&key);
                true
            } else if let Some(uses) = chains.def_use.get_mut(&key) {
                uses.retain(|usage| *usage != location);
                uses.is_empty()
            } else {
                false
            };
            if !became_empty {
                continue;
            }
            if !direct_removal {
                chains.def_use.remove(&key);
            }
            let Definition::Instruction(definition_location) = definition else {
                continue;
            };
            if live_locations.is_removed(definition_location) {
                continue;
            }
            let origin_location = locations.get(definition_location);
            let origin = &graph.nodes[origin_location.node].statements[origin_location.index];
            if origin.is_call() {
                graph.nodes[origin_location.node].statements[origin_location.index]
                    .remove_defined_variable();
            } else if !origin.has_side_effect() {
                update_chain(
                    graph,
                    locations,
                    live_locations,
                    chains,
                    definition_location,
                );
                remove_live_location(locations, live_locations, definition_location);
            }
        }
    }
}

fn remove_live_location(
    locations: &LocationIndex,
    live_locations: &mut ActiveLocations,
    location: Location,
) {
    live_locations.remove(locations, location);
}

fn register_propagation(
    graph: &mut Graph,
    locations: &LocationIndex,
    live_locations: &mut ActiveLocations,
    chains: &mut DefUseChains,
) {
    let mut used_variables = Vec::new();
    let mut origin_variables = Vec::new();
    let mut current_variables = Vec::new();
    let mut changed = true;
    while changed {
        changed = false;
        for rpo_index in 0..graph.rpo.len() {
            let node = graph.rpo[rpo_index];
            let mut cursor = 0;
            while cursor < live_locations.count(node) {
                let use_location = live_locations.select(locations, node, cursor);
                let statement_location = locations.get(use_location);
                used_variables.clear();
                graph.nodes[statement_location.node].statements[statement_location.index]
                    .collect_used_variables_vec(&mut used_variables);
                if used_variables.len() > 1 {
                    used_variables.sort();
                    used_variables.dedup();
                }
                for variable in used_variables.iter().copied() {
                    let Some(definition_location) = chains
                        .use_def
                        .get(&(variable, use_location))
                        .and_then(|definitions| match definitions.as_slice() {
                            [Definition::Instruction(location)] => Some(*location),
                            _ => None,
                        })
                    else {
                        continue;
                    };
                    if live_locations.is_removed(definition_location) {
                        continue;
                    }
                    let definition_statement = locations.get(definition_location);
                    let origin = &graph.nodes[definition_statement.node].statements
                        [definition_statement.index];
                    let Some(rhs) = origin.rhs() else {
                        continue;
                    };
                    if !origin.is_propagable() {
                        continue;
                    }
                    origin_variables.clear();
                    origin.collect_used_variables_vec(&mut origin_variables);
                    if origin_variables.len() > 1 {
                        origin_variables.sort();
                        origin_variables.dedup();
                    }
                    let definition_key =
                        (variable, Definition::Instruction(definition_location));
                    let is_constant = rhs.is_constant(&graph.variables);
                    let mut remove_single_use_definition = false;
                    if !is_constant {
                        if let Some(uses) = chains.def_use.get(&definition_key) {
                            if uses.len() > 1 {
                                continue;
                            }
                            remove_single_use_definition =
                                matches!(uses.as_slice(), [usage] if *usage == use_location);
                        }
                        if origin_variables.iter().copied().any(|used| {
                            !clear_path(
                                graph,
                                locations,
                                live_locations,
                                Some(used),
                                definition_location,
                                use_location,
                            )
                        }) {
                            continue;
                        }
                    }
                    if origin.has_side_effect()
                        && !clear_path(
                            graph,
                            locations,
                            live_locations,
                            None,
                            definition_location,
                            use_location,
                        )
                    {
                        continue;
                    }

                    let move_rhs = if remove_single_use_definition {
                        current_variables.clear();
                        graph.nodes[statement_location.node].statements
                            [statement_location.index]
                            .collect_used_variables_vec(&mut current_variables);
                        current_variables
                            .iter()
                            .filter(|used| **used == variable)
                            .take(2)
                            .count()
                            == 1
                    } else {
                        false
                    };
                    let mut removed_definition_uses = if move_rhs {
                        chains.def_use.remove(&definition_key)
                    } else {
                        None
                    };
                    let moved_rhs = removed_definition_uses.is_some()
                        && move_definition_rhs(
                            graph,
                            definition_statement,
                            statement_location,
                            variable,
                        );
                    if !moved_rhs {
                        if let Some(uses) = removed_definition_uses.take() {
                            chains.def_use.insert(definition_key, uses);
                        }
                        replace_with_definition_rhs(
                            graph,
                            definition_statement,
                            statement_location,
                            variable,
                        );
                    }
                    chains.use_def.remove(&(variable, use_location));
                    for origin_variable in origin_variables.iter().copied() {
                        let Some(old_definitions) = chains
                            .use_def
                            .remove(&(origin_variable, definition_location))
                        else {
                            continue;
                        };
                        for &old_definition in &old_definitions {
                            if let Some(uses) =
                                chains.def_use.get_mut(&(origin_variable, old_definition))
                            {
                                uses.retain(|usage| *usage != definition_location);
                                uses.push(use_location);
                            }
                        }
                        match chains.use_def.entry((origin_variable, use_location)) {
                            Entry::Vacant(entry) => {
                                entry.insert(old_definitions);
                            }
                            Entry::Occupied(mut entry) => {
                                entry.get_mut().extend(old_definitions);
                            }
                        }
                    }
                    if moved_rhs {
                        remove_live_location(
                            locations,
                            live_locations,
                            definition_location,
                        );
                        changed = true;
                    } else if remove_single_use_definition {
                        if chains.def_use.remove(&definition_key).is_some() {
                            remove_live_location(
                                locations,
                                live_locations,
                                definition_location,
                            );
                            changed = true;
                        }
                    } else if let Some(uses) = chains.def_use.get_mut(&definition_key) {
                        uses.retain(|usage| *usage != use_location);
                        if uses.is_empty() {
                            chains.def_use.remove(&definition_key);
                            remove_live_location(
                                locations,
                                live_locations,
                                definition_location,
                            );
                            changed = true;
                        }
                    }
                }
                cursor += 1;
            }
        }
    }
}

fn statement_pair_mut(
    graph: &mut Graph,
    first: StatementLocation,
    second: StatementLocation,
) -> (&mut Statement, &mut Statement) {
    assert_ne!((first.node, first.index), (second.node, second.index));
    if first.node == second.node {
        let statements = &mut graph.nodes[first.node].statements;
        if first.index < second.index {
            let (before_second, second_and_after) = statements.split_at_mut(second.index);
            (&mut before_second[first.index], &mut second_and_after[0])
        } else {
            let (before_first, first_and_after) = statements.split_at_mut(first.index);
            (&mut first_and_after[0], &mut before_first[second.index])
        }
    } else if first.node < second.node {
        let (before_second, second_and_after) = graph.nodes.split_at_mut(second.node);
        (
            &mut before_second[first.node].statements[first.index],
            &mut second_and_after[0].statements[second.index],
        )
    } else {
        let (before_first, first_and_after) = graph.nodes.split_at_mut(first.node);
        (
            &mut first_and_after[0].statements[first.index],
            &mut before_first[second.node].statements[second.index],
        )
    }
}

fn move_definition_rhs(
    graph: &mut Graph,
    definition: StatementLocation,
    usage: StatementLocation,
    variable: VariableId,
) -> bool {
    let (definition, usage) = statement_pair_mut(graph, definition, usage);
    let original = std::mem::replace(definition, Statement::Nop);
    let Statement::Assign { lhs, rhs, kind } = original else {
        *definition = original;
        return false;
    };
    if let Some(rhs) = usage.replace_once_owned(variable, rhs) {
        *definition = Statement::Assign { lhs, rhs, kind };
        false
    } else {
        true
    }
}

fn replace_with_definition_rhs(
    graph: &mut Graph,
    definition: StatementLocation,
    usage: StatementLocation,
    variable: VariableId,
) {
    let (definition, usage) = statement_pair_mut(graph, definition, usage);
    let rhs = definition
        .rhs()
        .expect("propagation origin has a right-hand side");
    usage.replace(variable, rhs);
}

pub fn place_declarations(graph: &mut Graph, chains: &DefUseChains) {
    let locations = LocationIndex::build(graph);
    place_declarations_at(graph, chains, &locations, None);
}

fn place_declarations_at(
    graph: &mut Graph,
    chains: &DefUseChains,
    locations: &LocationIndex,
    live_locations: Option<&ActiveLocations>,
) {
    let dominators = graph.immediate_dominators();
    let mut variables = Vec::new();
    for rpo_index in (0..graph.rpo.len()).rev() {
        let node = graph.rpo[rpo_index];
        for &use_location in locations.node_locations(node) {
            if live_locations.is_some_and(|live| live.is_removed(use_location)) {
                continue;
            }
            let statement_location = locations.get(use_location);
            collect_unique_used_variables(
                &graph.nodes[statement_location.node].statements[statement_location.index],
                &mut variables,
            );
            for &variable in &variables {
                if graph.variables.get(variable).is_none() {
                    continue;
                }
                if graph.variables.is_declared(variable) {
                    continue;
                }
                let Some(definitions) = chains.use_def.get(&(variable, use_location)) else {
                    continue;
                };
                let mut definition_nodes = Vec::with_capacity(definitions.len());
                for definition in definitions {
                    if let Definition::Instruction(location) = definition {
                        let definition_node = locations.node(*location);
                        if !graph.nodes[definition_node].in_catch {
                            definition_nodes.push(definition_node);
                        }
                    }
                }
                definition_nodes.sort_unstable();
                definition_nodes.dedup();
                let Some(mut common) = definition_nodes.pop() else {
                    continue;
                };
                for definition_node in definition_nodes {
                    common = common_dominator(graph, &dominators, common, definition_node);
                }
                let defined_in_common = definitions.iter().any(|definition| match definition {
                    Definition::Instruction(location) => locations.node(*location) == common,
                    Definition::Parameter(_) => false,
                });
                if !defined_in_common && !graph.nodes[common].declarations.contains(&variable) {
                    graph.nodes[common].declarations.push(variable);
                }
            }
        }
    }
}

fn clear_path(
    graph: &Graph,
    locations: &LocationIndex,
    live_locations: &ActiveLocations,
    variable: Option<VariableId>,
    from: Location,
    to: Location,
) -> bool {
    if from >= to {
        return false;
    }
    let from_location = locations.get(from);
    let to_location = locations.get(to);
    if from_location.node == to_location.node {
        return clear_statement_range(
            graph,
            locations,
            live_locations,
            from_location.node,
            from_location.index + 1,
            to_location.index,
            variable,
        );
    }
    if !clear_statement_range(
        graph,
        locations,
        live_locations,
        from_location.node,
        from_location.index + 1,
        graph.nodes[from_location.node].statements.len(),
        variable,
    ) {
        return false;
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([to_location.node]);
    while let Some(node) = queue.pop_front() {
        if node == from_location.node || !visited.insert(node) {
            continue;
        }
        let end = if node == to_location.node {
            to_location.index
        } else {
            graph.nodes[node].statements.len()
        };
        if !clear_statement_range(graph, locations, live_locations, node, 0, end, variable) {
            return false;
        }
        queue.extend(graph.all_predecessors(node));
    }
    true
}

fn clear_statement_range(
    graph: &Graph,
    locations: &LocationIndex,
    live_locations: &ActiveLocations,
    node: NodeId,
    start: usize,
    end: usize,
    variable: Option<VariableId>,
) -> bool {
    graph.nodes[node].statements[start..end]
        .iter()
        .enumerate()
        .all(|(relative, statement)| {
            let index = start + relative;
            let location = locations.node_locations(node)[index];
            if live_locations.is_removed(location) {
                return true;
            }
            statement.defined_variable() != variable && !statement.has_side_effect()
        })
}

fn common_dominator(
    graph: &Graph,
    dominators: &[Option<NodeId>],
    mut left: NodeId,
    mut right: NodeId,
) -> NodeId {
    while left != right {
        while graph.nodes[left].rpo > graph.nodes[right].rpo {
            left = dominators[left].expect("reachable node has a dominator");
        }
        while graph.nodes[right].rpo > graph.nodes[left].rpo {
            right = dominators[right].expect("reachable node has a dominator");
        }
    }
    left
}

fn compact_removed_without_index(
    graph: &mut Graph,
    locations: &LocationIndex,
    live_locations: &ActiveLocations,
) {
    for &node in &graph.rpo {
        let mut retained = Vec::with_capacity(graph.nodes[node].statements.len());
        for (index, statement) in graph.nodes[node].statements.drain(..).enumerate() {
            let location = locations.node_locations(node)[index];
            if !live_locations.is_removed(location) {
                retained.push(statement);
            }
        }
        graph.nodes[node].statements = retained;
    }
}

fn compact_removed(
    graph: &mut Graph,
    locations: &LocationIndex,
    live_locations: &ActiveLocations,
) -> LocationIndex {
    let mut compacted = LocationIndex {
        statements: Vec::with_capacity(live_locations.total_count()),
        by_node: vec![Vec::new(); graph.nodes.len()],
        has_instruction_definitions: locations.has_instruction_definitions,
    };
    for &node in &graph.rpo {
        let mut retained = Vec::with_capacity(graph.nodes[node].statements.len());
        for (index, statement) in graph.nodes[node].statements.drain(..).enumerate() {
            let location = locations.node_locations(node)[index];
            if !live_locations.is_removed(location) {
                compacted.by_node[node].push(compacted.statements.len());
                compacted.statements.push(StatementLocation {
                    node,
                    index: retained.len(),
                });
                retained.push(statement);
            }
        }
        graph.nodes[node].statements = retained;
    }
    compacted
}

fn definition_sort_key(definition: &Definition) -> (u8, usize) {
    match definition {
        Definition::Parameter(_) => (0, 0),
        Definition::Instruction(location) => (1, *location),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_word_range_mask_preserves_exact_bits_ut() {
        let mut words = Vec::new();
        append_word_range_mask(&mut words, 3..5);
        assert_eq!(words, vec![(0, 0b11 << 3)]);

        words.clear();
        append_word_range_mask(&mut words, 60..132);
        assert_eq!(words, vec![(0, !((1_u64 << 60) - 1)), (1, u64::MAX), (2, 0b1111)]);

        append_word_range_mask(&mut words, 12..12);
        assert_eq!(words, vec![(0, !((1_u64 << 60) - 1)), (1, u64::MAX), (2, 0b1111)]);
    }

    #[test]
    fn clear_bit_range_preserves_boundary_bits_ut() {
        let mut bits = [u64::MAX; 3];
        clear_bit_range(&mut bits, 3..5);
        assert_eq!(bits, [u64::MAX ^ (0b11 << 3), u64::MAX, u64::MAX]);

        let mut spanning_bits = [u64::MAX; 3];
        clear_bit_range(&mut spanning_bits, 60..132);
        assert_eq!(spanning_bits[0], (1_u64 << 60) - 1);
        assert_eq!(spanning_bits[1], 0);
        assert_eq!(spanning_bits[2], !0b1111);

        let before = bits;
        clear_bit_range(&mut bits, 12..12);
        assert_eq!(bits, before);
    }

    #[test]
    fn parameter_only_chains_have_no_instruction_definitions() {
        let variable = VariableId::Register(0);
        let parameter = Definition::Parameter(variable);
        let instruction = Definition::Instruction(1);
        let mut chains = DefUseChains::default();

        chains.def_use.insert((variable, parameter), vec![0]);
        chains.use_def.insert((variable, 0), vec![parameter]);
        assert!(!chains_have_instruction_definitions(&chains));

        chains.def_use.insert((variable, instruction), vec![2]);
        chains.use_def.insert((variable, 2), vec![instruction]);
        assert!(chains_have_instruction_definitions(&chains));
    }
}
