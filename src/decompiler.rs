use crate::control_flow;
use crate::dataflow;
use crate::dex::{
    ClassDef, DexFile, EncodedAnnotation, EncodedField, EncodedMethod, EncodedValue,
};
use crate::error::{Error, Result};
use crate::graph::Graph;
use crate::ir::opcode::lower_method_with_resolved;
use crate::types::{class_access, descriptor_to_java, field_access, method_access};
use crate::writer::{escape_java_string, MemberKind, MemberReference, MethodWriter, SourceOutput};

pub struct Decompiler<'a> {
    dex: DexFile<'a>,
    include_metadata: bool,
}

impl<'a> Decompiler<'a> {
    pub fn parse(dex_bytes: &'a [u8]) -> Result<Self> {
        Ok(Self {
            dex: DexFile::parse(dex_bytes)?,
            include_metadata: true,
        })
    }

    pub(crate) fn parse_owned(dex_bytes: std::sync::Arc<[u8]>) -> Result<Decompiler<'static>> {
        Ok(Decompiler {
            dex: DexFile::parse_owned(dex_bytes)?,
            include_metadata: true,
        })
    }

    /// Disables DEX annotations in rendered source while retaining member-reference metadata.
    pub const fn without_metadata(mut self) -> Self {
        self.include_metadata = false;
        self
    }

    pub fn dex(&self) -> &DexFile<'a> {
        &self.dex
    }

    pub fn decompile_class(&self, dalvik_class: &str) -> Result<Option<SourceOutput>> {
        let Some(class) = self.dex.class(dalvik_class) else {
            return Ok(None);
        };
        self.write_class(class).map(Some)
    }

    pub fn decompile_all(&self) -> Result<Vec<(String, SourceOutput)>> {
        self.dex
            .classes()
            .iter()
            .map(|class| {
                Ok((
                    self.dex.class_descriptor(class)?.to_owned(),
                    self.write_class(class)?,
                ))
            })
            .collect()
    }

    /// Decompile one scheduler-owned class range. APK pipeline workers use this
    /// primitive so every DEX shares one global class-worker pool rather than
    /// creating nested pools per DEX.
    pub(crate) fn decompile_class_range_for_export<F>(
        &self,
        range: std::ops::Range<usize>,
        mut visit: F,
    ) -> Result<usize>
    where
        F: FnMut(&str, SourceOutput) -> Result<()>,
    {
        let classes = self
            .dex
            .classes()
            .get(range.clone())
            .ok_or_else(|| Error::data(0, "APK class job has an invalid class range"))?;
        for class in classes {
            let descriptor = self.dex.class_descriptor(class)?;
            let output = self.write_class(class)?;
            visit(descriptor, output)?;
        }
        Ok(range.len())
    }

    pub fn decompile_all_parallel(
        &self,
        thread_count: usize,
    ) -> Result<Vec<(String, SourceOutput)>> {
        let classes = self.dex.classes();
        if thread_count <= 1 || classes.len() <= 1 {
            return self.decompile_all();
        }

        let worker_count = thread_count.min(classes.len());
        let chunk_size = classes.len().div_ceil(worker_count);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for chunk in classes.chunks(chunk_size) {
                handles.push(scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|class| {
                            Ok((
                                self.dex.class_descriptor(class)?.to_owned(),
                                self.write_class(class)?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()
                }));
            }

            let mut output = Vec::with_capacity(classes.len());
            for handle in handles {
                let chunk = handle
                    .join()
                    .map_err(|_| Error::data(0, "parallel decompiler worker panicked"))??;
                output.extend(chunk);
            }
            Ok(output)
        })
    }

    fn write_class(&self, class: &ClassDef) -> Result<SourceOutput> {
        self.write_class_into(SourceOutput::default(), class)
    }

    fn write_class_into(
        &self,
        mut output: SourceOutput,
        class: &ClassDef,
    ) -> Result<SourceOutput> {
        let descriptor = self.dex.class_descriptor(class)?;
        let (package, name) = class_parts(descriptor);
        if !package.is_empty() {
            output.source.push_str("package ");
            output.source.push_str(&package);
            output.source.push_str(";\n");
        }

        let interface = class.access_flags & 0x200 != 0;
        let access_flags = if interface {
            class.access_flags & !0x400
        } else {
            class.access_flags
        };
        output
            .source
            .push_str(&class_access(access_flags).join(" "));
        output.source.push(' ');
        if !interface {
            output.source.push_str("class ");
        }
        output.source.push_str(&name);

        if let Some(superclass) = self.dex.class_superclass(class)?
            && superclass != "Ljava/lang/Object;"
        {
            output.source.push_str(" extends ");
            output
                .source
                .push_str(&superclass[1..superclass.len() - 1].replace('/', "."));
        }
        let interfaces = self.dex.class_interfaces(class)?;
        if !interfaces.is_empty() {
            output.source.push_str(" implements ");
            for (index, interface) in interfaces.iter().enumerate() {
                if index != 0 {
                    output.source.push_str(", ");
                }
                output
                    .source
                    .push_str(&interface[1..interface.len() - 1].replace('/', "."));
            }
        }
        output.source.push_str(" {\n");

        for field in class.fields() {
            self.write_field(&mut output, field)?;
        }
        let method_annotations = if self.include_metadata {
            Some(self.dex.method_annotations(class)?)
        } else {
            None
        };
        for method in class.methods() {
            let annotations = method_annotations
                .as_ref()
                .and_then(|annotations| annotations.get(&method.method_idx))
                .map(Vec::as_slice)
                .unwrap_or_default();
            output = self.write_method(output, descriptor, &name, method, annotations)?;
        }
        output.source.push_str("}\n");
        Ok(output)
    }

    fn write_field(&self, output: &mut SourceOutput, field: &EncodedField) -> Result<()> {
        let resolved = self.dex.field(field.field_idx)?;
        output.source.push_str("    ");
        let access = field_access(field.access_flags);
        if !access.is_empty() {
            output.source.push_str(&access.join(" "));
            output.source.push(' ');
        }
        output.source.push_str(&descriptor_to_java(resolved.ty));
        output.source.push(' ');
        let start = output.source.len();
        output.source.push_str(resolved.name);
        output.member_references.push(MemberReference {
            start,
            end: output.source.len(),
            kind: MemberKind::Field,
            class: resolved.class.to_owned(),
            name: resolved.name.to_owned(),
            descriptor: resolved.ty.to_owned(),
            declaration: true,
        });
        if let Some(value) = &field.initial_value {
            output.source.push_str(" = ");
            output.source.push_str(&self.encoded_value(value, resolved.ty)?);
        }
        output.source.push_str(";\n");
        Ok(())
    }

    fn write_method(
        &self,
        mut output: SourceOutput,
        class_descriptor: &str,
        class_name: &str,
        method: &EncodedMethod,
        annotations: &[EncodedAnnotation],
    ) -> Result<SourceOutput> {
        let resolved = self.dex.method(method.method_idx)?;
        let return_type = resolved.return_type;
        let access = method_access(method.access_flags);
        let constructor = access.iter().any(|value| value == "constructor");
        let modifiers: Vec<_> = access
            .iter()
            .filter(|value| value.as_str() != "constructor")
            .map(String::as_str)
            .collect();
        for annotation in annotations {
            output.source.push_str("\n    ");
            output.source.push_str(&self.annotation(annotation)?);
        }
        output.source.push_str("\n    ");
        if !modifiers.is_empty() {
            output.source.push_str(&modifiers.join(" "));
            output.source.push(' ');
        }
        if !constructor {
            output.source.push_str(&descriptor_to_java(return_type));
            output.source.push(' ');
        }
        let displayed_name = if constructor {
            class_name
        } else {
            resolved.name
        };
        let name_start = output.source.len();
        output.source.push_str(displayed_name);
        output.member_references.push(MemberReference {
            start: name_start,
            end: output.source.len(),
            kind: MemberKind::Method,
            class: class_descriptor.to_owned(),
            name: resolved.name.to_owned(),
            descriptor: resolved.descriptor.to_owned(),
            declaration: true,
        });

        let lowered = lower_method_with_resolved(&self.dex, method, resolved)?;
        let mut graph = lowered.map(Graph::from_lowered);
        let parameter_registers = graph
            .as_ref()
            .map(|graph| {
                let mut parameters: Vec<_> = graph
                    .variables
                    .parameter_ids()
                    .into_iter()
                    .filter(|variable| !graph.variables.is_this(*variable))
                    .collect();
                parameters.sort();
                parameters
            })
            .unwrap_or_default();
        output.source.push('(');
        for (index, (parameter_type_index, parameter)) in resolved
            .parameter_type_indices
            .iter()
            .zip(parameter_registers.iter())
            .enumerate()
        {
            let parameter_type = self.dex.type_descriptor(*parameter_type_index)?;
            if index != 0 {
                output.source.push_str(", ");
            }
            output
                .source
                .push_str(&descriptor_to_java(parameter_type));
            output.source.push(' ');
            output.source.push('p');
            let name = graph
                .as_ref()
                .and_then(|graph| graph.variables.name(*parameter))
                .unwrap_or("unknown");
            output.source.push_str(name);
        }
        output.source.push(')');

        let Some(graph) = graph.as_mut() else {
            output.source.push_str(";\n");
            return Ok(output);
        };
        dataflow::optimize_for_decompilation(graph);
        graph.split_condition_nodes();
        graph.simplify();
        graph.compute_rpo();
        control_flow::identify_structures(graph);
        Ok(
            MethodWriter::new_moving_conditions(
                graph,
                class_descriptor,
                constructor,
                output,
            )
            .write_body(),
        )
    }

    fn encoded_value(&self, value: &EncodedValue, field_type: &str) -> Result<String> {
        let rendered = match value {
            EncodedValue::Byte(value) => {
                if field_type == "B" {
                    signed_hex(i64::from(*value))
                } else {
                    value.to_string()
                }
            }
            EncodedValue::Short(value) => value.to_string(),
            EncodedValue::Char(value) => value.to_string(),
            EncodedValue::Int(value) => value.to_string(),
            EncodedValue::Long(value) => value.to_string(),
            EncodedValue::Float(bits) => f32::from_bits(*bits).to_string(),
            EncodedValue::Double(bits) => f64::from_bits(*bits).to_string(),
            EncodedValue::String(index) => escape_java_string(self.dex.string(*index)?),
            EncodedValue::Type(index) => {
                format!("{}.class", descriptor_to_java(self.dex.type_descriptor(*index)?))
            }
            EncodedValue::Field(index) | EncodedValue::Enum(index) => {
                let field = self.dex.field(*index)?;
                format!("{}.{}", descriptor_to_java(field.class), field.name)
            }
            EncodedValue::Method(index) => {
                let method = self.dex.method(*index)?;
                format!("{}.{}", descriptor_to_java(method.class), method.name)
            }
            EncodedValue::MethodType(index) => self.dex.proto_descriptor(*index)?,
            EncodedValue::MethodHandle(index) => index.to_string(),
            EncodedValue::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| self.encoded_value(value, field_type))
                    .collect::<Result<Vec<_>>>()?;
                format!("{{{}}}", values.join(", "))
            }
            EncodedValue::Annotation(annotation) => {
                let name = descriptor_to_java(self.dex.type_descriptor(annotation.type_idx)?);
                let mut elements = Vec::with_capacity(annotation.elements.len());
                for element in &annotation.elements {
                    elements.push(format!(
                        "{}={}",
                        self.dex.string(element.name_idx)?,
                        self.encoded_value(&element.value, field_type)?
                    ));
                }
                if elements.is_empty() {
                    format!("@{name}")
                } else {
                    format!("@{name}({})", elements.join(", "))
                }
            }
            EncodedValue::Null if field_type == "Ljava/lang/String;" => "\"\"".to_owned(),
            EncodedValue::Null => "None".to_owned(),
            EncodedValue::Boolean(value) => {
                if *value {
                    "True".to_owned()
                } else {
                    "False".to_owned()
                }
            }
        };
        Ok(rendered)
    }

    fn annotation(&self, annotation: &EncodedAnnotation) -> Result<String> {
        let name = annotation_type_name(self.dex.type_descriptor(annotation.type_idx)?);
        if annotation.elements.is_empty() {
            return Ok(format!("@{name}"));
        }
        let mut elements = Vec::with_capacity(annotation.elements.len());
        for element in &annotation.elements {
            elements.push(format!(
                "{}={}",
                self.dex.string(element.name_idx)?,
                self.annotation_value(&element.value)?
            ));
        }
        Ok(format!("@{name}({})", elements.join(", ")))
    }

    fn annotation_value(&self, value: &EncodedValue) -> Result<String> {
        Ok(match value {
            EncodedValue::String(index) => escape_java_string(self.dex.string(*index)?),
            EncodedValue::Type(index) => format!(
                "{}.class",
                annotation_type_name(self.dex.type_descriptor(*index)?)
            ),
            EncodedValue::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| self.annotation_value(value))
                    .collect::<Result<Vec<_>>>()?;
                format!("{{{}}}", values.join(", "))
            }
            EncodedValue::Annotation(annotation) => self.annotation(annotation)?,
            EncodedValue::Boolean(value) => {
                if *value {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            EncodedValue::Null => "null".to_owned(),
            _ => self.encoded_value(value, "")?,
        })
    }
}

pub fn decompile_dex_bytes(
    dex_bytes: &[u8],
    dalvik_class_fmt: &str,
) -> Result<SourceOutput> {
    let decompiler = Decompiler::parse(dex_bytes)?;
    Ok(match decompiler.decompile_class(dalvik_class_fmt)? {
        Some(output) => output,
        None => SourceOutput {
            source: format!(
                "Error: Class {dalvik_class_fmt} not found in the reconstructed DEX."
            ),
            member_references: Vec::new(),
        },
    })
}

fn class_parts(descriptor: &str) -> (String, String) {
    let body = descriptor
        .strip_prefix('L')
        .and_then(|value| value.strip_suffix(';'))
        .unwrap_or(descriptor);
    if let Some((package, name)) = body.rsplit_once('/') {
        (package.replace('/', "."), name.to_owned())
    } else {
        (String::new(), body.to_owned())
    }
}

fn signed_hex(value: i64) -> String {
    if value < 0 {
        format!("-0x{:x}", value.unsigned_abs())
    } else {
        format!("0x{value:x}")
    }
}

fn annotation_type_name(descriptor: &str) -> String {
    descriptor
        .strip_prefix('L')
        .and_then(|value| value.strip_suffix(';'))
        .unwrap_or(descriptor)
        .replace('/', ".")
        .rsplit_once('.')
        .map(|(_, name)| name)
        .unwrap_or(descriptor)
        .replace('$', ".")
}
