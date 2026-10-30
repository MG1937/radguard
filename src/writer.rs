use crate::graph::{ConditionTree, Graph, LoopKind, NodeId};
use crate::ir::opcode::BlockKind;
use crate::ir::{
    AssignKind, BinaryOp, Constant, ConstantValue, FieldReference, InvokeKind, MethodReference,
    Statement, UnaryOp, Value, VariableId,
};
use crate::types::descriptor_to_java;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Method,
    Field,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberReference {
    pub start: usize,
    pub end: usize,
    pub kind: MemberKind,
    pub class: String,
    pub name: String,
    pub descriptor: String,
    pub declaration: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SourceOutput {
    pub source: String,
    pub member_references: Vec<MemberReference>,
}

impl SourceOutput {
    pub fn append(&mut self, mut other: Self) {
        let base = self.source.len();
        for reference in &mut other.member_references {
            reference.start += base;
            reference.end += base;
        }
        self.source.push_str(&other.source);
        self.member_references.extend(other.member_references);
    }
}

pub struct MethodWriter<'a> {
    graph: &'a Graph,
    debug: bool,
    output: SourceOutput,
    declared: Vec<bool>,
    visited: Vec<bool>,
    conditions: Vec<Option<ConditionTree>>,
    negated_condition_leaves: Vec<bool>,
    indentation: usize,
    loop_follow: Vec<Option<NodeId>>,
    if_follow: Vec<Option<NodeId>>,
    switch_follow: Vec<Option<NodeId>>,
    latch_node: Vec<Option<NodeId>>,
    try_follow: Vec<Option<NodeId>>,
    next_case: Option<NodeId>,
    need_break: bool,
    constructor: bool,
    current_class: &'a str,
    this_is_super: bool,
}

impl<'a> MethodWriter<'a> {
    pub fn new(graph: &'a Graph, current_class: &'a str, constructor: bool) -> Self {
        let conditions = graph
            .nodes
            .iter()
            .map(|node| node.condition.clone())
            .collect();
        Self::with_conditions(
            graph,
            current_class,
            constructor,
            conditions,
            SourceOutput::default(),
            false,
        )
    }

    pub(crate) fn new_moving_conditions(
        graph: &'a mut Graph,
        current_class: &'a str,
        constructor: bool,
        output: SourceOutput,
        debug: bool,
    ) -> Self {
        let conditions = graph
            .nodes
            .iter_mut()
            .map(|node| node.condition.take())
            .collect();
        Self::with_conditions(graph, current_class, constructor, conditions, output, debug)
    }

    fn with_conditions(
        graph: &'a Graph,
        current_class: &'a str,
        constructor: bool,
        conditions: Vec<Option<ConditionTree>>,
        output: SourceOutput,
        debug: bool,
    ) -> Self {
        Self {
            graph,
            debug,
            output,
            declared: {
                let mut declared = vec![false; graph.variables.dense_len()];
                for variable in graph.variables.parameter_ids() {
                    declared[graph.variables.dense_index(variable).expect("known parameter")] = true;
                }
                declared
            },
            visited: vec![false; graph.nodes.len()],
            conditions,
            negated_condition_leaves: vec![false; graph.nodes.len()],
            indentation: 4,
            loop_follow: vec![None],
            if_follow: vec![None],
            switch_follow: vec![None],
            latch_node: vec![None],
            try_follow: vec![None],
            next_case: None,
            need_break: true,
            constructor,
            current_class,
            this_is_super: false,
        }
    }

    pub fn write_body(mut self) -> SourceOutput {
        self.write("\n");
        self.write_indent();
        self.write("{\n");
        self.indentation += 4;
        self.visit_node(self.graph.entry);
        self.indentation -= 4;
        self.write_indent();
        self.write("}\n");
        self.output
    }

    fn visit_node(&mut self, node: NodeId) {
        if self
            .if_follow
            .last()
            .into_iter()
            .chain(self.switch_follow.last())
            .chain(self.loop_follow.last())
            .chain(self.latch_node.last())
            .chain(self.try_follow.last())
            .any(|follow| *follow == Some(node))
        {
            return;
        }
        if self.graph.nodes[node].kind != BlockKind::Return && self.visited[node] {
            return;
        }
        self.visited[node] = true;
        let declarations = self.graph.nodes[node].declarations.clone();
        for variable in declarations {
            self.write_declaration(variable);
        }

        match self.graph.nodes[node].kind {
            BlockKind::Statement => self.visit_statement_node(node),
            BlockKind::Return => {
                self.need_break = false;
                self.visit_statements(node);
            }
            BlockKind::Throw => self.visit_statements(node),
            BlockKind::Condition | BlockKind::ShortCircuit => self.visit_condition_node(node),
            BlockKind::Switch => self.visit_switch_node(node),
            BlockKind::Loop => self.visit_loop_node(node),
            BlockKind::Try => self.visit_try_node(node),
            BlockKind::Catch => self.visit_catch_node(node),
        }
    }

    fn visit_loop_node(&mut self, node: NodeId) {
        let loop_node = &self.graph.nodes[node];
        if self.debug {
            eprintln!(
                "[radguard-debug] writer loop node={} kind={:?} wrapped={:?} latch={:?} follow={:?} branches=({:?},{:?})",
                node,
                loop_node.loop_kind,
                loop_node.wrapped,
                loop_node.latch,
                loop_node.loop_follow,
                loop_node.branch_true,
                loop_node.branch_false
            );
        }
        let follow = loop_node.loop_follow;
        let wrapped = loop_node.wrapped.expect("loop wrapper has a condition");
        let mut true_branch = loop_node.branch_true;
        let mut false_branch = loop_node.branch_false;
        let mut negate = false;

        match loop_node.loop_kind {
            LoopKind::PreTest => {
                if true_branch == follow {
                    negate = true;
                    std::mem::swap(&mut true_branch, &mut false_branch);
                }
                self.write_indent();
                self.write("while (");
                self.write_condition(wrapped, negate);
                self.write(") {\n");
            }
            LoopKind::PostTest => {
                self.write_indent();
                self.write("do {\n");
                self.latch_node.push(loop_node.latch);
            }
            LoopKind::Endless | LoopKind::None => {
                self.write_indent();
                self.write("while(true) {\n");
            }
        }

        self.indentation += 4;
        self.loop_follow.push(follow);
        if loop_node.loop_kind == LoopKind::PreTest {
            if let Some(target) = true_branch {
                self.visit_node(target);
            }
        } else {
            self.visit_node(wrapped);
        }
        self.loop_follow.pop();
        self.indentation -= 4;

        match loop_node.loop_kind {
            LoopKind::PreTest => {
                self.write_indent();
                self.write("}\n");
            }
            LoopKind::PostTest => {
                self.latch_node.pop();
                self.write_indent();
                self.write("} while(");
                if let Some(latch) = loop_node.latch {
                    self.write_wrapped_condition(latch, false);
                }
                self.write(");\n");
            }
            LoopKind::Endless | LoopKind::None => {
                if let Some(latch) = loop_node.latch {
                    self.indentation += 4;
                    self.visit_node(latch);
                    self.indentation -= 4;
                }
                self.write_indent();
                self.write("}\n");
            }
        }
        if let Some(follow) = follow {
            self.visit_node(follow);
        }
    }

    fn visit_condition_node(&mut self, node: NodeId) {
        let condition = &self.graph.nodes[node];
        if self.debug {
            eprintln!(
                "[radguard-debug] writer condition node={} follow={:?} branches=({:?},{:?}) statements={:?}",
                node,
                condition.if_follow,
                condition.branch_true,
                condition.branch_false,
                condition.statements
            );
        }
        let follow = condition.if_follow;
        let mut true_branch = condition.branch_true;
        let mut false_branch = condition.branch_false;
        let mut negate = false;

        if true_branch == false_branch {
            self.write_indent();
            self.write("// Both branches of the condition point to the same code.\n");
            self.write_indent();
            self.write("// if (");
            self.write_condition(node, false);
            self.write(") {\n");
            self.indentation += 4;
            if let Some(target) = true_branch {
                self.visit_node(target);
            }
            self.indentation -= 4;
            self.write_indent();
            self.write("// }\n");
            return;
        }
        if false_branch == *self.loop_follow.last().unwrap_or(&None) {
            negate = true;
            std::mem::swap(&mut true_branch, &mut false_branch);
        }
        let loop_follow = *self.loop_follow.last().unwrap_or(&None);
        if loop_follow == true_branch || loop_follow == false_branch {
            self.write_indent();
            self.write("if (");
            self.write_condition(node, negate);
            self.write(") {\n");
            self.indentation += 4;
            self.write_indent();
            self.write("break;\n");
            self.indentation -= 4;
            self.write_indent();
            self.write("}\n");
            if let Some(target) = false_branch {
                self.visit_node(target);
            }
            return;
        }

        if let Some(follow) = follow {
            if true_branch == Some(follow)
                || true_branch == self.next_case
                || true_branch.is_some_and(|target| condition.rpo > self.graph.nodes[target].rpo)
            {
                negate = !negate;
                std::mem::swap(&mut true_branch, &mut false_branch);
            }
            self.if_follow.push(Some(follow));
            if let Some(target) = true_branch {
                self.write_indent();
                self.write("if (");
                self.write_condition(node, negate);
                self.write(") {\n");
                self.indentation += 4;
                self.visit_node(target);
                self.indentation -= 4;
            }
            let is_else = true_branch != Some(follow) && false_branch != Some(follow);
            if is_else
                && false_branch.is_some_and(|target| !self.visited[target])
            {
                self.write_indent();
                self.write("} else {\n");
                self.indentation += 4;
                self.visit_node(false_branch.expect("checked branch"));
                self.indentation -= 4;
            }
            self.if_follow.pop();
            self.write_indent();
            self.write("}\n");
            self.visit_node(follow);
        } else {
            self.write_indent();
            self.write("if (");
            self.write_condition(node, negate);
            self.write(") {\n");
            self.indentation += 4;
            if let Some(target) = true_branch {
                self.visit_node(target);
            }
            self.indentation -= 4;
            self.write_indent();
            self.write("} else {\n");
            self.indentation += 4;
            if let Some(target) = false_branch {
                self.visit_node(target);
            }
            self.indentation -= 4;
            self.write_indent();
            self.write("}\n");
        }
    }

    fn visit_switch_node(&mut self, node: NodeId) {
        let switch = &self.graph.nodes[node];
        for statement in switch.statements.iter().take(switch.statements.len().saturating_sub(1)) {
            self.write_statement(statement);
        }
        self.write_indent();
        self.write("switch (");
        if let Some(Statement::Switch { value, .. }) = switch.statements.last() {
            self.write_value(value);
        }
        self.write(") {\n");
        let follow = switch.switch_follow;
        self.switch_follow.push(follow);

        let mut default = switch.switch_default;
        for (index, (_, target)) in switch.switch_cases.iter().enumerate() {
            let target = *target;
            if self.visited[target] {
                continue;
            }
            self.indentation += 4;
            for (value, case_target) in &switch.switch_cases {
                if *case_target == target {
                    self.write_indent();
                    self.write("case ");
                    self.write_i64(i64::from(*value));
                    self.write(":\n");
                }
            }
            self.next_case = switch.switch_cases.get(index + 1).map(|(_, target)| *target);
            if default == Some(target) {
                self.write_indent();
                self.write("default:\n");
                default = None;
            }
            self.indentation += 4;
            self.visit_node(target);
            if self.need_break {
                self.write_indent();
                self.write("break;\n");
            } else {
                self.need_break = true;
            }
            self.indentation -= 8;
        }
        if let Some(default) = default
            && Some(default) != follow
        {
            self.indentation += 4;
            self.write_indent();
            self.write("default:\n");
            self.indentation += 4;
            self.visit_node(default);
            self.indentation -= 8;
        }
        self.write_indent();
        self.write("}\n");
        self.switch_follow.pop();
        if let Some(follow) = follow {
            self.visit_node(follow);
        }
    }

    fn visit_statement_node(&mut self, node: NodeId) {
        self.visit_statements(node);
        let successors = &self.graph.nodes[node].successors;
        if let [successor] = successors.as_slice() {
            if Some(*successor) == *self.loop_follow.last().unwrap_or(&None) {
                self.write_indent();
                self.write("break;\n");
            } else if Some(*successor) == self.next_case {
                self.need_break = false;
            } else {
                self.visit_node(*successor);
            }
        }
    }

    fn visit_try_node(&mut self, node: NodeId) {
        let try_node = &self.graph.nodes[node];
        self.write_indent();
        self.write("try {\n");
        self.indentation += 4;
        self.try_follow.push(try_node.try_follow);
        if let Some(start) = try_node.wrapped {
            self.visit_node(start);
        }
        self.indentation -= 4;
        self.write_indent();
        self.write("}");
        for catch in &try_node.catches {
            self.visit_node(*catch);
        }
        self.write("\n");
        let follow = self.try_follow.pop().flatten();
        if let Some(follow) = follow {
            self.visit_node(follow);
        }
    }

    fn visit_catch_node(&mut self, node: NodeId) {
        let catch = &self.graph.nodes[node];
        self.write(" catch (");
        let start = catch.wrapped.expect("catch wrapper has a start");
        let statements = self.graph.nodes[start].statements.as_slice();
        if catch.catch_types.len() > 1 {
            let types = catch
                .catch_types
                .iter()
                .map(|ty| descriptor_to_java(ty))
                .collect::<Vec<_>>()
                .join(" | ");
            self.write(&types);
            if let Some(Statement::MoveException { lhs, .. }) = statements.first() {
                self.write(" ");
                self.write_variable_name(*lhs);
            }
        } else if let Some(Statement::MoveException { lhs, ty }) = statements.first() {
            let variable_type = self
                .graph
                .variables
                .get(*lhs)
                .and_then(|variable| variable.ty.as_deref());
            let ty = catch
                .catch_types
                .first()
                .map(String::as_str)
                .or(ty.as_deref())
                .or(variable_type)
                .unwrap_or("Ljava/lang/Throwable;");
            self.write(&descriptor_to_java(ty));
            self.write(" ");
            self.write_variable_name(*lhs);
        } else {
            let provenance_type = catch.catch_order.and_then(|token| {
                let source = (token >> 32) as usize;
                let handler = token as u32 as usize;
                self.graph.nodes.get(source).and_then(|node| {
                    node.catch_handlers
                        .get(handler)
                        .and_then(|(_, ty)| ty.as_deref())
                })
            });
            self.write(&descriptor_to_java(
                provenance_type
                    .or_else(|| catch.catch_types.first().map(String::as_str))
                    .unwrap_or("Ljava/lang/Throwable;"),
            ));
        }
        self.write(") {\n");
        self.indentation += 4;
        self.visit_node(start);
        self.indentation -= 4;
        self.write_indent();
        self.write("}");
    }

    fn visit_statements(&mut self, node: NodeId) {
        let statements = &self.graph.nodes[node].statements;
        let first = usize::from(
            self.graph.nodes[node].catch_target
                && !self.graph.nodes[node].catch_source
                && matches!(statements.first(), Some(Statement::MoveException { .. })),
        );
        for statement in &statements[first..] {
            self.write_statement(statement);
        }
    }

    fn write_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Assign { lhs, rhs, kind } => {
                if let Some(lhs) = lhs {
                    if *kind == AssignKind::Move && matches!(rhs, Value::Variable(rhs) if rhs == lhs)
                    {
                        return;
                    }
                    if *kind != AssignKind::MoveResult && self.write_in_place(*lhs, rhs) {
                        return;
                    }
                    if self.should_skip_constructor(rhs) {
                        return;
                    }
                    self.write_indent();
                    if is_void_super_invoke(rhs) {
                        self.mark_declared(*lhs);
                    } else {
                        self.write_variable(*lhs);
                        self.write(" = ");
                    }
                    self.write_value(rhs);
                    self.write(";\n");
                } else if !self.should_skip_constructor(rhs) {
                    self.write_indent();
                    self.write_value(rhs);
                    self.write(";\n");
                }
            }
            Statement::ArrayStore(store) => {
                self.write_indent();
                self.write_value(&store.array);
                self.write("[");
                self.write_value(&store.index);
                self.write("] = ");
                self.write_value(&store.value);
                self.write(";\n");
            }
            Statement::StaticPut { value, field } => {
                self.write_indent();
                self.write_static_field(field, false);
                self.write(" = ");
                self.write_value(value);
                self.write(";\n");
            }
            Statement::InstancePut(store) => {
                self.write_indent();
                self.write_value(&store.object);
                self.write(".");
                self.write_field_name(&store.field, false);
                self.write(" = ");
                self.write_value(&store.value);
                self.write(";\n");
            }
            Statement::Return(value) => {
                self.write_indent();
                self.write("return");
                if let Some(value) = value {
                    self.write(" ");
                    self.write_value(value);
                }
                self.write(";\n");
            }
            Statement::Nop | Statement::Switch { .. } | Statement::Condition { .. } => {}
            Statement::FillArray {
                array,
                element_width,
                size,
                data,
            } => {
                self.write_indent();
                self.write_value(array);
                self.write(" = {");
                self.write(&format_array_data(*element_width, *size, data));
                self.write("};\n");
            }
            Statement::MoveException { lhs, ty } => {
                self.write(
                    &descriptor_to_java(ty.as_deref().unwrap_or("Ljava/lang/Throwable;")),
                );
                self.write(" ");
                self.write_variable_name(*lhs);
            }
            Statement::MonitorEnter(value) => {
                self.write_indent();
                self.write("synchronized(");
                self.write_value(value);
                self.write(") {\n");
                self.indentation += 4;
            }
            Statement::MonitorExit(_) => {
                self.indentation = self.indentation.saturating_sub(4);
                self.write_indent();
                self.write("}\n");
            }
            Statement::Throw(value) => {
                self.write_indent();
                self.write("throw ");
                self.write_value(value);
                self.write(";\n");
            }
        }
    }

    fn write_in_place(&mut self, lhs: VariableId, rhs: &Value) -> bool {
        let Value::Binary(binary) = rhs else {
            return false;
        };
        if !matches!(&binary.left, Value::Variable(variable) if *variable == lhs) {
            return false;
        }
        self.write_indent();
        self.write_variable(lhs);
        if matches!(binary.op, BinaryOp::Add | BinaryOp::Subtract)
            && matches!(
                &binary.right,
                Value::Constant(Constant {
                    value: ConstantValue::Integer { value: 1, .. },
                })
            )
        {
            self.write(if binary.op == BinaryOp::Add { "++;\n" } else { "--;\n" });
        } else {
            self.write(" ");
            self.write(binary_operator(binary.op));
            self.write("= ");
            self.write_value(&binary.right);
            self.write(";\n");
        }
        true
    }

    fn write_wrapped_condition(&mut self, mut node: NodeId, negate: bool) {
        while self.conditions[node].is_none()
            && !self.graph.nodes[node]
                .statements
                .iter()
                .any(|statement| matches!(statement, Statement::Condition { .. }))
        {
            let Some(wrapped) = self.graph.nodes[node].wrapped else {
                break;
            };
            node = wrapped;
        }
        self.write_condition(node, negate);
    }

    fn write_condition(&mut self, node: NodeId, negate: bool) {
        if let Some(mut condition) = self.conditions[node].take() {
            if negate {
                self.negate_condition_tree(&mut condition);
            }
            self.write_condition_tree(&mut condition);
            self.conditions[node] = Some(condition);
            return;
        }
        if negate {
            self.toggle_condition_leaf(node);
        }
        self.write_condition_leaf(node);
    }

    fn write_condition_leaf(&mut self, node: NodeId) {
        let condition = self.graph.nodes[node]
            .statements
            .iter()
            .rev()
            .find(|statement| matches!(statement, Statement::Condition { .. }));
        let Some(Statement::Condition { op, left, right }) = condition else {
            self.write("false");
            return;
        };
        let op = if self.negated_condition_leaves[node] {
            op.negate()
        } else {
            *op
        };
        if let Some(right) = right {
            self.write_value(left);
            self.write(" ");
            self.write(binary_operator(op));
            self.write(" ");
            self.write_value(right);
        } else if let Value::Binary(binary) = left
            && binary.op == BinaryOp::Compare
        {
            self.write_value(&binary.left);
            self.write(" ");
            self.write(binary_operator(op));
            self.write(" ");
            self.write_value(&binary.right);
        } else if value_type(left, self.graph).is_some_and(|ty| ty == "Z") {
            if op == BinaryOp::Equal {
                self.write("!");
            }
            self.write_value(left);
        } else {
            self.write_value(left);
            self.write(" ");
            self.write(binary_operator(op));
            self.write(" ");
            if value_type(left, self.graph)
                .is_some_and(|ty| matches!(ty, "V" | "B" | "S" | "C" | "I" | "J" | "F" | "D"))
            {
                self.write("0");
            } else {
                self.write("null");
            }
        }
    }

    fn negate_condition_tree(&mut self, condition: &mut ConditionTree) {
        match condition {
            ConditionTree::Leaf(node) => self.toggle_condition_leaf(*node),
            ConditionTree::Compound {
                left,
                right,
                is_and,
                ..
            } => {
                *is_and = !*is_and;
                self.negate_condition_tree(left);
                self.negate_condition_tree(right);
            }
        }
    }

    fn toggle_condition_leaf(&mut self, node: NodeId) {
        self.negated_condition_leaves[node] = !self.negated_condition_leaves[node];
    }

    fn write_condition_tree(&mut self, condition: &mut ConditionTree) {
        match condition {
            ConditionTree::Leaf(node) => self.write_condition_leaf(*node),
            ConditionTree::Compound {
                left,
                right,
                is_and,
                is_not,
            } => {
                if *is_not {
                    self.negate_condition_tree(left);
                }
                self.write("(");
                self.write_condition_tree(left);
                self.write(if *is_and { ") && (" } else { ") || (" });
                self.write_condition_tree(right);
                self.write(")");
            }
        }
    }

    fn write_value(&mut self, value: &Value) {
        match value {
            Value::Variable(variable) => self.write_variable(*variable),
            Value::Constant(constant) => self.write_constant(constant),
            Value::BaseClass { name, .. } => self.write(name),
            Value::NewInstance { ty } => {
                self.write("new ");
                self.write(&descriptor_to_java(ty));
            }
            Value::Invoke(invoke) => {
                self.write_invoke(&invoke.method, &invoke.base, &invoke.arguments)
            }
            Value::CheckCast(cast) => {
                self.write("((");
                self.write(&cast.ty);
                self.write(") ");
                self.write_value(&cast.value);
                self.write(")");
            }
            Value::ArrayLoad(load) => {
                self.write_value(&load.array);
                self.write("[");
                self.write_value(&load.index);
                self.write("]");
            }
            Value::ArrayLength(array) => {
                self.write_value(array);
                self.write(".length");
            }
            Value::NewArray(array) => {
                self.write("new ");
                self.write(&descriptor_to_java(
                    array.ty.strip_prefix('[').unwrap_or(&array.ty),
                ));
                self.write("[");
                self.write_value(&array.size);
                self.write("]");
            }
            Value::FilledArray(array) => {
                self.write("new ");
                self.write(&descriptor_to_java(&array.ty));
                self.write(" {");
                for (index, argument) in array.arguments.iter().enumerate() {
                    if index != 0 {
                        self.write(", ");
                    }
                    self.write_value(argument);
                }
                self.write("})");
            }
            Value::Binary(binary) => {
                let parenthesized = binary.op != BinaryOp::Compare;
                if parenthesized {
                    self.write("(");
                }
                self.write_value(&binary.left);
                self.write(" ");
                self.write(binary_operator(binary.op));
                self.write(" ");
                self.write_value(&binary.right);
                if parenthesized {
                    self.write(")");
                }
            }
            Value::Unary(unary) => {
                self.write("(");
                match unary.op {
                    UnaryOp::Negate => self.write("- "),
                    UnaryOp::Not => self.write("~ "),
                    UnaryOp::Cast => {
                        self.write(unary.cast.unwrap_or(""));
                        self.write(" ");
                    }
                }
                self.write_value(&unary.value);
                self.write(")");
            }
            Value::InstanceField(field) => {
                self.write_value(&field.object);
                self.write(".");
                self.write_field_name(&field.field, false);
            }
            Value::StaticField(field) => self.write_static_field(field, false),
        }
    }

    fn write_invoke(&mut self, method: &MethodReference, base: &Value, arguments: &[Value]) {
        let constructor = method.name == "<init>";
        if constructor && self.is_this(base) && method.class != self.current_class {
            self.this_is_super = true;
            self.write("super");
        } else {
            self.write_value(base);
        }
        if !constructor {
            self.write(".");
            let start = self.output.source.len();
            self.write(&method.name);
            self.output.member_references.push(MemberReference {
                start,
                end: self.output.source.len(),
                kind: MemberKind::Method,
                class: method.class.clone(),
                name: method.name.clone(),
                descriptor: method_descriptor(method),
                declaration: false,
            });
        }
        self.write("(");
        for (index, argument) in arguments.iter().enumerate() {
            if index != 0 {
                self.write(", ");
            }
            self.write_value(argument);
        }
        self.write(")");
    }

    fn should_skip_constructor(&self, value: &Value) -> bool {
        matches!(
            value,
            Value::Invoke(invoke) if self.constructor
                && invoke.method.name == "<init>"
                && invoke.arguments.is_empty()
                && self.is_this(&invoke.base)
        )
    }

    fn is_this(&self, value: &Value) -> bool {
        matches!(
            value,
            Value::Variable(variable) if self.graph.variables.is_this(*variable)
        )
    }

    fn write_constant(&mut self, constant: &Constant) {
        match &constant.value {
            ConstantValue::String(value) => self.write(&escape_java_string(value)),
            ConstantValue::Integer { value, ty: "Z" } => {
                self.write(if *value == 0 { "false" } else { "true" });
            }
            ConstantValue::Integer { value, .. } => self.write_i64(*value),
        }
    }

    fn write_variable(&mut self, variable: VariableId) {
        let info = self
            .graph
            .variables
            .get(variable)
            .expect("IR references a registered variable");
        if self.graph.variables.is_this(variable) {
            self.write(if self.this_is_super { "super" } else { "this" });
            return;
        }
        if self.graph.variables.is_super(variable) {
            self.write("super");
            return;
        }
        if self.graph.variables.is_declared(variable) {
            self.write("p");
            match variable {
                VariableId::Register(register) => self.write_u64(u64::from(register)),
                _ => self.write(
                    self.graph
                        .variables
                        .name(variable)
                        .expect("registered variable has a name"),
                ),
            }
            return;
        }
        if !self.is_declared(variable) {
            self.mark_declared(variable);
            self.write(&descriptor_to_java(info.ty.as_deref().unwrap_or("unknownType")));
            self.write(" ");
        }
        self.write_variable_name(variable);
    }

    fn is_declared(&self, variable: VariableId) -> bool {
        self.graph
            .variables
            .dense_index(variable)
            .is_some_and(|index| self.declared[index])
    }

    fn mark_declared(&mut self, variable: VariableId) {
        if let Some(index) = self.graph.variables.dense_index(variable) {
            self.declared[index] = true;
        }
    }

    fn write_variable_name(&mut self, variable: VariableId) {
        self.output.source.push('v');
        match self.graph.variables.name(variable) {
            Some(name) => self.output.source.push_str(name),
            None => self.output.source.push_str("unknown"),
        }
        self.mark_declared(variable);
    }

    fn write_declaration(&mut self, variable: VariableId) {
        if self.is_declared(variable) {
            return;
        }
        let info = self
            .graph
            .variables
            .get(variable)
            .expect("declaration references a registered variable");
        self.write_indent();
        self.write(&descriptor_to_java(info.ty.as_deref().unwrap_or("unknownType")));
        self.write(" ");
        self.write_variable_name(variable);
        self.write(";\n");
    }

    fn write_static_field(&mut self, field: &FieldReference, declaration: bool) {
        self.write(&descriptor_to_java(field.class.as_ref()));
        self.write(".");
        self.write_field_name(field, declaration);
    }

    fn write_field_name(&mut self, field: &FieldReference, declaration: bool) {
        let start = self.output.source.len();
        self.write(field.name.as_ref());
        self.output.member_references.push(MemberReference {
            start,
            end: self.output.source.len(),
            kind: MemberKind::Field,
            class: field.class.to_string(),
            name: field.name.to_string(),
            descriptor: field.ty.to_string(),
            declaration,
        });
    }

    fn write_indent(&mut self) {
        self.output
            .source
            .extend(std::iter::repeat_n(' ', self.indentation));
    }

    fn write(&mut self, value: &str) {
        self.output.source.push_str(value);
    }

    fn write_u64(&mut self, mut value: u64) {
        let mut digits = [0_u8; 20];
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        // Decimal digits are always valid UTF-8.
        self.write(unsafe { std::str::from_utf8_unchecked(&digits[start..]) });
    }

    fn write_i64(&mut self, value: i64) {
        if value < 0 {
            self.write("-");
        }
        self.write_u64(value.unsigned_abs());
    }
}

pub fn binary_operator(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Compare => "cmp",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::And => "&",
        BinaryOp::Or => "|",
        BinaryOp::Xor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight | BinaryOp::UnsignedShiftRight => ">>",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Less => "<",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Greater => ">",
        BinaryOp::LessEqual => "<=",
        BinaryOp::InstanceOf => "instanceof",
    }
}

pub fn escape_java_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\'' | '"' | '\\' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            ' '..='~' => escaped.push(character),
            character => escaped.push_str(&format!("\\u{:04x}", u32::from(character))),
        }
    }
    escaped.push('"');
    escaped
}

fn value_type<'a>(value: &'a Value, graph: &'a Graph) -> Option<&'a str> {
    match value {
        Value::Variable(variable) => graph
            .variables
            .get(*variable)
            .and_then(|variable| variable.ty.as_deref()),
        Value::Constant(constant) => Some(constant.ty()),
        Value::NewInstance { ty } => Some(ty),
        Value::NewArray(array) => Some(&array.ty),
        Value::FilledArray(array) => Some(&array.ty),
        Value::CheckCast(cast) => Some(&cast.descriptor),
        Value::Binary(binary) => Some(binary.ty),
        Value::Unary(unary) => Some(unary.ty),
        Value::Invoke(invoke) => Some(&invoke.method.return_type),
        Value::ArrayLength(_) => Some("I"),
        Value::ArrayLoad(load) => value_type(&load.array, graph)
            .and_then(|ty| ty.strip_prefix('['))
            .or(load.ty),
        Value::InstanceField(field) => Some(field.field.ty.as_ref()),
        Value::StaticField(field) => Some(field.ty.as_ref()),
        Value::BaseClass { .. } => None,
    }
}

fn is_void_super_invoke(value: &Value) -> bool {
    matches!(
        value,
        Value::Invoke(invoke)
            if invoke.kind == InvokeKind::Super && invoke.method.return_type == "V"
    )
}

fn method_descriptor(method: &MethodReference) -> String {
    format!(
        "({}){}",
        method.parameters.join(" "),
        method.return_type
    )
}

fn format_array_data(element_width: u16, size: u32, data: &[u8]) -> String {
    let width = match element_width {
        1 | 2 | 4 | 8 => usize::from(element_width),
        _ => 1,
    };
    let count = usize::try_from(size).unwrap_or(usize::MAX);
    data.chunks_exact(width)
        .take(count)
        .map(|bytes| match width {
            1 => i8::from_le_bytes([bytes[0]]).to_string(),
            2 => i16::from_le_bytes([bytes[0], bytes[1]]).to_string(),
            4 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string(),
            8 => format_dad_f64(f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_dad_f64(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value == f64::INFINITY {
        return "inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".to_owned();
    }
    let mut rendered = format!("{value:?}");
    if let Some(exponent) = rendered.find('e') {
        let has_sign = matches!(rendered.as_bytes()[exponent + 1], b'+' | b'-');
        let exponent_digits = rendered.len() - exponent - 1 - usize::from(has_sign);
        if !has_sign {
            rendered.insert(exponent + 1, '+');
        }
        if exponent_digits < 2 {
            rendered.insert(exponent + 2, '0');
        }
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::format_dad_f64;

    #[test]
    fn dad_f64_format_preserves_float_identity_bits_ut() {
        assert_eq!(format_dad_f64(-90.0), "-90.0");
        assert_eq!(format_dad_f64(1228.8), "1228.8");
        assert_eq!(format_dad_f64(7.411e-320), "7.411e-320");
        assert_eq!(format_dad_f64(1.0e16), "1e+16");
        assert_eq!(format_dad_f64(1.0e-5), "1e-05");
        assert_eq!(format_dad_f64(-0.0), "-0.0");
        assert_eq!(format_dad_f64(f64::NAN), "nan");
        assert_eq!(format_dad_f64(f64::INFINITY), "inf");
        assert_eq!(format_dad_f64(f64::NEG_INFINITY), "-inf");
    }
}
