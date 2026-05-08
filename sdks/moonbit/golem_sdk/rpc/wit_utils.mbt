///|
pub fn unwrap_wit_tuple(value : @types.WitValue) -> @types.WitValue {
  guard value.nodes.length() > 0 else { return value }
  guard value.nodes[0] is @types.WitNode::TupleValue(indices) else {
    return value
  }
  guard indices.length() == 1 else { return value }
  let root_index = indices[0]
  let offset = root_index
  let new_nodes : Array[@types.WitNode] = []
  for i = root_index; i < value.nodes.length(); i = i + 1 {
    new_nodes.push(rebase_node(value.nodes[i], offset))
  }
  @types.WitValue::{ nodes: new_nodes }
}

///|
fn rebase_node(node : @types.WitNode, offset : Int) -> @types.WitNode {
  match node {
    RecordValue(indices) =>
      @types.WitNode::RecordValue(indices.map(fn(i) { i - offset }))
    TupleValue(indices) =>
      @types.WitNode::TupleValue(indices.map(fn(i) { i - offset }))
    ListValue(indices) =>
      @types.WitNode::ListValue(indices.map(fn(i) { i - offset }))
    VariantValue((disc, child)) =>
      @types.WitNode::VariantValue((disc, child.map(fn(i) { i - offset })))
    OptionValue(child) =>
      @types.WitNode::OptionValue(child.map(fn(i) { i - offset }))
    ResultValue(r) =>
      @types.WitNode::ResultValue(
        match r {
          Ok(v) => Ok(v.map(fn(i) { i - offset }))
          Err(v) => Err(v.map(fn(i) { i - offset }))
        },
      )
    other => other
  }
}

///|
pub fn[T : @schema.ToElementValue] to_wit_value(
  v : T,
) -> @types.WitValue raise @common.AgentError {
  let element = T::to_element_value(v)
  guard element is @types.ElementValue::ComponentModel(wv) else {
    raise @common.AgentError::InvalidInput(
      "RPC transport only supports ComponentModel values, not UnstructuredText/UnstructuredBinary. Use ComponentModel types for RPC parameters.",
    )
  }
  wv
}

///|
pub fn[T : @schema.FromElementValue] from_wit_value(
  wv : @types.WitValue,
) -> T raise @common.AgentError {
  T::from_element_value(@types.ElementValue::ComponentModel(wv))
}

///|
/// Encodes a `Multimodal[T]` as a WitValue `list<variant>` for RPC transport.
/// Each modality item becomes a variant case in the list, where:
/// - ComponentModel modalities are encoded as regular WitValues
/// - UnstructuredText modalities are encoded as text-reference WitValues
/// - UnstructuredBinary modalities are encoded as binary-reference WitValues
pub fn[T : @agentTypes.MultimodalModality] multimodal_to_wit_value(
  multimodal : @agentTypes.Multimodal[T],
) -> @types.WitValue raise @common.AgentError {
  let schema = T::multimodal_schema()
  // Pre-encode all items outside the builder callback, since the builder
  // callback only allows BuilderError, not AgentError
  let encoded : Array[(UInt, @types.WitValue)] = []
  for item in multimodal.items {
    let (name, element_value) = T::to_named_element(item)
    let case_idx = find_modality_case_index(schema, name)
    let payload_wv = element_value_to_wit_value(element_value)
    encoded.push((case_idx, payload_wv))
  }
  let wv = @builder.Builder::new().list(lb => {
    for pair in encoded {
      let (case_idx, payload_wv) = pair
      lb.variant(case_idx, inner => inner.wit_value(payload_wv))
    }
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  wv
}

///|
/// Decodes a WitValue `list<variant>` back to `Multimodal[T]`.
pub fn[T : @agentTypes.MultimodalModality] wit_value_to_multimodal(
  wv : @types.WitValue,
) -> @agentTypes.Multimodal[T] raise @common.AgentError {
  let schema = T::multimodal_schema()
  guard wv.nodes.length() > 0 else {
    raise @common.AgentError::InvalidInput("Empty WitValue for multimodal")
  }
  guard wv.nodes[0] is @types.WitNode::ListValue(variant_indices) else {
    raise @common.AgentError::InvalidInput(
      "Expected list for multimodal WitValue",
    )
  }
  let items : Array[T] = []
  for i = 0; i < variant_indices.length(); i = i + 1 {
    let variant_idx = variant_indices[i]
    guard wv.nodes[variant_idx]
      is @types.WitNode::VariantValue((case_idx, payload_idx_opt)) else {
      raise @common.AgentError::InvalidInput(
        "Expected variant in multimodal list",
      )
    }
    let case_idx_int = case_idx.reinterpret_as_int()
    guard case_idx_int < schema.length() else {
      raise @common.AgentError::InvalidInput(
        "Invalid multimodal variant case index: " + case_idx.to_string(),
      )
    }
    let (name, element_schema) = schema[case_idx_int]
    guard payload_idx_opt is Some(payload_idx) else {
      raise @common.AgentError::InvalidInput(
        "Expected payload for multimodal variant case",
      )
    }
    let end_idx = if i + 1 < variant_indices.length() {
      variant_indices[i + 1]
    } else {
      wv.nodes.length()
    }
    let sub_wv = extract_sub_tree(wv, payload_idx, end_idx)
    let element_value = wit_value_to_element_value(sub_wv, element_schema)
    items.push(T::from_named_element(name, element_value))
  }
  @agentTypes.Multimodal::new(items)
}

///|
fn find_modality_case_index(
  schema : Array[(String, @types.ElementSchema)],
  name : String,
) -> UInt raise @common.AgentError {
  for i = 0; i < schema.length(); i = i + 1 {
    if schema[i].0 == name {
      return i.reinterpret_as_uint()
    }
  }
  raise @common.AgentError::InvalidInput("Unknown modality name: " + name)
}

///|
fn element_value_to_wit_value(
  ev : @types.ElementValue,
) -> @types.WitValue raise @common.AgentError {
  match ev {
    @types.ElementValue::ComponentModel(wv) => wv
    @types.ElementValue::UnstructuredText(text_ref) =>
      unstructured_text_to_wit_value(
        text_reference_to_unstructured_text(text_ref),
      )
    @types.ElementValue::UnstructuredBinary(bin_ref) =>
      unstructured_binary_to_wit_value(
        binary_reference_to_unstructured_binary(bin_ref),
      )
  }
}

///|
fn wit_value_to_element_value(
  wv : @types.WitValue,
  element_schema : @types.ElementSchema,
) -> @types.ElementValue raise @common.AgentError {
  match element_schema {
    @types.ElementSchema::ComponentModel(_) =>
      @types.ElementValue::ComponentModel(wv)
    @types.ElementSchema::UnstructuredText(_) => {
      let ut = wit_value_to_unstructured_text(wv)
      @schema.to_element_value_as(ut)
    }
    @types.ElementSchema::UnstructuredBinary(_) => {
      let ub = wit_value_to_unstructured_binary(wv)
      @schema.to_element_value_as(ub)
    }
  }
}

///|
fn extract_sub_tree(
  wv : @types.WitValue,
  start_idx : Int,
  end_idx : Int,
) -> @types.WitValue {
  let offset = start_idx
  let new_nodes : Array[@types.WitNode] = []
  for i = start_idx; i < end_idx; i = i + 1 {
    new_nodes.push(rebase_node(wv.nodes[i], offset))
  }
  @types.WitValue::{ nodes: new_nodes }
}

///|
fn text_reference_to_unstructured_text(
  ref_ : @types.TextReference,
) -> @agentTypes.UnstructuredText {
  match ref_ {
    @types.TextReference::Url(url) => @agentTypes.UnstructuredText::Url(url)
    @types.TextReference::Inline(source) =>
      @agentTypes.UnstructuredText::Inline(
        text=source.data,
        language_code=source.text_type.map(fn(t) { t.language_code }),
      )
  }
}

///|
fn binary_reference_to_unstructured_binary(
  ref_ : @types.BinaryReference,
) -> @agentTypes.UnstructuredBinary {
  match ref_ {
    @types.BinaryReference::Url(url) => @agentTypes.UnstructuredBinary::Url(url)
    @types.BinaryReference::Inline(source) =>
      @agentTypes.UnstructuredBinary::Inline(
        data=Bytes::from_array(source.data[:]),
        mime_type=source.binary_type.mime_type,
      )
  }
}

///|
/// Encodes an `UnstructuredText` as a `WitValue` matching the WIT `text-reference` variant:
///
/// ```wit
/// variant text-reference { url(string), inline(text-source) }
/// record text-source { data: string, text-type: option<text-type> }
/// record text-type { language-code: string }
/// ```
pub fn unstructured_text_to_wit_value(
  text : @agentTypes.UnstructuredText,
) -> @types.WitValue raise @common.AgentError {
  let wv = match text {
    Url(url) =>
      @builder.Builder::new().variant_option(0, Some(b => b.string(url))) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
    Inline(text~, language_code~) =>
      @builder.Builder::new().variant_option(
        1,
        Some(b => {
          b.record(rec => {
            rec.string(text)
            match language_code {
              Some(lc) =>
                rec.option_some(b => b.record(inner => inner.string(lc)))
              None => rec.option_none()
            }
          })
        }),
      ) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
  }
  wv
}

///|
/// Decodes a `WitValue` (encoded as `text-reference` variant) back to `UnstructuredText`.
pub fn wit_value_to_unstructured_text(
  wv : @types.WitValue,
) -> @agentTypes.UnstructuredText raise @common.AgentError {
  let ext = @extractor.extract(wv)
  guard ext.variant() is Some((disc, inner)) else {
    raise @common.AgentError::InvalidInput(
      "Expected variant for text-reference",
    )
  }
  match disc {
    0 => {
      guard inner is Some(e) else {
        raise @common.AgentError::InvalidInput(
          "Expected payload for text-reference url",
        )
      }
      guard e.string() is Some(url) else {
        raise @common.AgentError::InvalidInput(
          "Expected string for text-reference url",
        )
      }
      @agentTypes.UnstructuredText::Url(url)
    }
    1 => {
      guard inner is Some(source) else {
        raise @common.AgentError::InvalidInput(
          "Expected payload for text-reference inline",
        )
      }
      guard source.field(0) is Some(data_ext) else {
        raise @common.AgentError::InvalidInput(
          "Expected field 0 (data) in text-source",
        )
      }
      guard data_ext.string() is Some(data) else {
        raise @common.AgentError::InvalidInput(
          "Expected string for text-source data",
        )
      }
      guard source.field(1) is Some(tt_ext) else {
        raise @common.AgentError::InvalidInput(
          "Expected field 1 (text-type) in text-source",
        )
      }
      guard tt_ext.option() is Some(text_type_opt) else {
        raise @common.AgentError::InvalidInput(
          "Expected option for text-source text-type",
        )
      }
      let language_code = text_type_opt.map(fn(e) {
        e.field(0).unwrap().string().unwrap()
      })
      @agentTypes.UnstructuredText::Inline(text=data, language_code~)
    }
    _ =>
      raise @common.AgentError::InvalidInput(
        "Invalid text-reference variant discriminant: " + disc.to_string(),
      )
  }
}

///|
/// Encodes an `UnstructuredBinary` as a `WitValue` matching the WIT `binary-reference` variant:
///
/// ```wit
/// variant binary-reference { url(string), inline(binary-source) }
/// record binary-source { data: list<u8>, binary-type: binary-type }
/// record binary-type { mime-type: string }
/// ```
pub fn unstructured_binary_to_wit_value(
  binary : @agentTypes.UnstructuredBinary,
) -> @types.WitValue raise @common.AgentError {
  let wv = match binary {
    Url(url) =>
      @builder.Builder::new().variant_option(0, Some(b => b.string(url))) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
    Inline(data~, mime_type~) =>
      @builder.Builder::new().variant_option(
        1,
        Some(b => {
          b.record(rec => {
            rec.list(lb => {
              for i = 0; i < data.length(); i = i + 1 {
                lb.u8(data[i])
              }
            })
            rec.record(inner => inner.string(mime_type))
          })
        }),
      ) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
  }
  wv
}

///|
/// Decodes a `WitValue` (encoded as `binary-reference` variant) back to `UnstructuredBinary`.
pub fn wit_value_to_unstructured_binary(
  wv : @types.WitValue,
) -> @agentTypes.UnstructuredBinary raise @common.AgentError {
  let ext = @extractor.extract(wv)
  guard ext.variant() is Some((disc, inner)) else {
    raise @common.AgentError::InvalidInput(
      "Expected variant for binary-reference",
    )
  }
  match disc {
    0 => {
      guard inner is Some(e) else {
        raise @common.AgentError::InvalidInput(
          "Expected payload for binary-reference url",
        )
      }
      guard e.string() is Some(url) else {
        raise @common.AgentError::InvalidInput(
          "Expected string for binary-reference url",
        )
      }
      @agentTypes.UnstructuredBinary::Url(url)
    }
    1 => {
      guard inner is Some(source) else {
        raise @common.AgentError::InvalidInput(
          "Expected payload for binary-reference inline",
        )
      }
      guard source.field(0) is Some(data_ext) else {
        raise @common.AgentError::InvalidInput(
          "Expected field 0 (data) in binary-source",
        )
      }
      guard data_ext.list_elements() is Some(elements) else {
        raise @common.AgentError::InvalidInput(
          "Expected list for binary-source data",
        )
      }
      let byte_array : Array[Byte] = []
      for i = 0; i < elements.length(); i = i + 1 {
        guard elements[i].u8() is Some(b) else {
          raise @common.AgentError::InvalidInput(
            "Expected u8 element in binary-source data",
          )
        }
        byte_array.push(b)
      }
      let bytes = Bytes::from_array(byte_array[:])
      guard source.field(1) is Some(bt_ext) else {
        raise @common.AgentError::InvalidInput(
          "Expected field 1 (binary-type) in binary-source",
        )
      }
      guard bt_ext.field(0) is Some(mt_ext) else {
        raise @common.AgentError::InvalidInput(
          "Expected field 0 (mime-type) in binary-type",
        )
      }
      guard mt_ext.string() is Some(mime_type) else {
        raise @common.AgentError::InvalidInput(
          "Expected string for binary-type mime-type",
        )
      }
      @agentTypes.UnstructuredBinary::Inline(data=bytes, mime_type~)
    }
    _ =>
      raise @common.AgentError::InvalidInput(
        "Invalid binary-reference variant discriminant: " + disc.to_string(),
      )
  }
}

///|
/// Creates a `TypedAgentConfigValue` from a path and a typed value.
/// The value type must be a ComponentModel type (not unstructured text/binary).
pub fn[T : @schema.ToElementValue + @schema.HasElementSchema] typed_config_value(
  path : Array[String],
  v : T,
) -> @common.TypedAgentConfigValue raise @common.AgentError {
  let wv = to_wit_value(v)
  let es = T::element_schema()
  guard es is @types.ElementSchema::ComponentModel(wt) else {
    raise @common.AgentError::InvalidInput(
      "Config values must use ComponentModel types, not unstructured types",
    )
  }
  { path, value: { value: wv, typ: wt } }
}
