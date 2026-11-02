//! Run the quantized model's matrix products in f32.
//!
//! `onnx/model_quantized.onnx` is the dynamically quantized export of the
//! model: each weight matrix is stored as 8-bit integers with a scale and a
//! zero point, and each product quantizes its activations at run time —
//! `DynamicQuantizeLinear → MatMulInteger → Cast → Mul(scale)`. tract runs
//! integer products several times slower than its f32 ones, and they do not
//! speed up with matmul threads; quantizing the activations also costs
//! fidelity. [`dequantize_matmuls`] rewrites each chain into one f32 `MatMul`
//! over its weights dequantized once, at load: the download stays the 162 MB
//! quantized file, and only the weights' own quantization is left. Measured
//! on 600 entities: 2.15× the throughput of the integer products, 0.5 GB more
//! resident, and vectors closer to the unquantized model's (mean cosine 0.9963
//! against 0.9919). See docs/decisions.md.

use std::collections::{HashMap, HashSet};

use tract_onnx::pb;

use crate::error::{Error, Result};

/// ONNX data types of the tensors read here.
const FLOAT: i32 = 1;
const UINT8: i32 = 2;
const INT8: i32 = 3;

/// The rewrite of one quantized product.
struct Rewrite {
    /// The `MatMulInteger` replaced.
    product: usize,
    /// The `Cast` and the `Mul` by the scales, now unused.
    dropped: [usize; 2],
    matmul: pb::NodeProto,
    weight: pb::TensorProto,
}

/// Rewrite every `DynamicQuantizeLinear → MatMulInteger → Cast → Mul` chain
/// of `model` into `MatMul(x, W)`, with `W = (W_q − W_zp) · W_scale` computed
/// here, and drop the nodes and initializers that nothing uses any more. A
/// chain of another shape is left as it is. Returns how many products were
/// rewritten, and how many quantized ones the graph had.
///
/// # Errors
/// Returns [`Error::Embedding`] if the model has no graph.
pub fn dequantize_matmuls(model: &mut pb::ModelProto) -> Result<(usize, usize)> {
    let graph = model
        .graph
        .as_mut()
        .ok_or_else(|| Error::Embedding("the model has no graph".to_string()))?;
    let quantized = graph
        .node
        .iter()
        .filter(|node| node.op_type == "MatMulInteger")
        .count();
    let rewrites: Vec<Rewrite> = {
        let index = GraphIndex::new(graph);
        (0..graph.node.len())
            .filter(|&i| graph.node[i].op_type == "MatMulInteger")
            .filter_map(|i| rewrite(graph, &index, i))
            .collect()
    };
    let rewritten = rewrites.len();
    if rewritten == 0 {
        return Ok((0, quantized));
    }

    let mut replace: HashMap<usize, pb::NodeProto> = HashMap::new();
    let mut dropped: HashSet<usize> = HashSet::new();
    for rewrite in rewrites {
        replace.insert(rewrite.product, rewrite.matmul);
        dropped.extend(rewrite.dropped);
        graph.initializer.push(rewrite.weight);
    }
    // Each `MatMul` takes its `MatMulInteger`'s place, which keeps the nodes
    // in topological order: its inputs come before, its consumers after.
    let nodes = std::mem::take(&mut graph.node);
    graph.node = nodes
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !dropped.contains(i))
        .map(|(i, node)| replace.remove(&i).unwrap_or(node))
        .collect();
    drop_unused(graph);
    Ok((rewritten, quantized))
}

/// Who produces and who consumes each value of a graph, and its initializers.
struct GraphIndex<'a> {
    initializers: HashMap<&'a str, &'a pb::TensorProto>,
    producer: HashMap<&'a str, usize>,
    consumers: HashMap<&'a str, Vec<usize>>,
}

impl<'a> GraphIndex<'a> {
    fn new(graph: &'a pb::GraphProto) -> Self {
        let mut index = Self {
            initializers: graph
                .initializer
                .iter()
                .map(|t| (t.name.as_str(), t))
                .collect(),
            producer: HashMap::new(),
            consumers: HashMap::new(),
        };
        for (i, node) in graph.node.iter().enumerate() {
            for output in &node.output {
                index.producer.insert(output.as_str(), i);
            }
            for input in &node.input {
                index.consumers.entry(input.as_str()).or_default().push(i);
            }
        }
        index
    }

    /// The one node that consumes `value`, if it is one of `op_type`.
    fn only_consumer(&self, graph: &pb::GraphProto, value: &str, op_type: &str) -> Option<usize> {
        match self.consumers.get(value).map(Vec::as_slice) {
            Some(&[i]) if graph.node[i].op_type == op_type => Some(i),
            _ => None,
        }
    }
}

/// The rewrite of the `MatMulInteger` at `product`, if its chain has the
/// shape the quantizer produces and its weights are constants.
fn rewrite(graph: &pb::GraphProto, index: &GraphIndex<'_>, product: usize) -> Option<Rewrite> {
    let node = &graph.node[product];
    let quantizer = &graph.node[*index.producer.get(node.input.first()?.as_str())?];
    if quantizer.op_type != "DynamicQuantizeLinear" {
        return None;
    }
    let (x, x_scale) = (quantizer.input.first()?, quantizer.output.get(1)?);

    let cast = index.only_consumer(graph, node.output.first()?, "Cast")?;
    let cast_output = graph.node[cast].output.first()?;
    let scaled = index.only_consumer(graph, cast_output, "Mul")?;
    let scales = graph.node[scaled]
        .input
        .iter()
        .find(|v| *v != cast_output)?;
    let scale_product = &graph.node[*index.producer.get(scales.as_str())?];
    if scale_product.op_type != "Mul" {
        return None;
    }
    let w_scale = scale_product.input.iter().find(|v| *v != x_scale)?;

    let w_q = index.initializers.get(node.input.get(1)?.as_str())?;
    let &[k, n] = w_q.dims.as_slice() else {
        return None;
    };
    let (k, n) = (usize::try_from(k).ok()?, usize::try_from(n).ok()?);
    let quantized = values(w_q)?;
    let zero_points = match node.input.get(3).filter(|name| !name.is_empty()) {
        Some(name) => values(index.initializers.get(name.as_str())?)?,
        None => vec![0.0],
    };
    let scale = values(index.initializers.get(w_scale.as_str())?)?;
    let per_column = |v: &[f32], c: usize| match v.len() {
        1 => Some(v[0]),
        len if len == n => Some(v[c]),
        _ => None,
    };
    if quantized.len() != k * n {
        return None;
    }
    let mut raw = Vec::with_capacity(k * n * 4);
    for (i, &q) in quantized.iter().enumerate() {
        let c = i % n;
        let w = (q - per_column(&zero_points, c)?) * per_column(&scale, c)?;
        raw.extend_from_slice(&w.to_le_bytes());
    }

    let weight_name = format!("{}/dequantized", w_q.name);
    Some(Rewrite {
        product,
        dropped: [cast, scaled],
        matmul: pb::NodeProto {
            name: format!("{}/f32", node.name),
            op_type: "MatMul".to_string(),
            input: vec![x.clone(), weight_name.clone()],
            output: graph.node[scaled].output.clone(),
            ..pb::NodeProto::default()
        },
        weight: pb::TensorProto {
            name: weight_name,
            dims: vec![k as i64, n as i64],
            data_type: FLOAT,
            raw_data: raw,
            ..pb::TensorProto::default()
        },
    })
}

/// The values of a constant `f32`, `u8` or `i8` tensor.
fn values(tensor: &pb::TensorProto) -> Option<Vec<f32>> {
    let raw = &tensor.raw_data;
    match tensor.data_type {
        FLOAT if !raw.is_empty() => Some(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        ),
        FLOAT => Some(tensor.float_data.clone()),
        UINT8 if !raw.is_empty() => Some(raw.iter().map(|&b| f32::from(b)).collect()),
        INT8 if !raw.is_empty() => Some(raw.iter().map(|&b| f32::from(b as i8)).collect()),
        // Without raw data, ONNX keeps 8-bit values in `int32_data`.
        UINT8 | INT8 => Some(tensor.int32_data.iter().map(|&v| v as f32).collect()),
        _ => None,
    }
}

/// Drop the nodes whose outputs nothing uses (the quantizers and scale
/// products a rewrite leaves), and the initializers, inputs and value
/// annotations of what is gone.
fn drop_unused(graph: &mut pb::GraphProto) {
    let outputs: HashSet<String> = graph.output.iter().map(|o| o.name.clone()).collect();
    loop {
        let used: HashSet<&str> = graph
            .node
            .iter()
            .flat_map(|node| node.input.iter().map(String::as_str))
            .collect();
        let live: Vec<bool> = graph
            .node
            .iter()
            .map(|node| {
                node.output
                    .iter()
                    .any(|o| used.contains(o.as_str()) || outputs.contains(o))
            })
            .collect();
        if live.iter().all(|&l| l) {
            break;
        }
        let nodes = std::mem::take(&mut graph.node);
        graph.node = nodes
            .into_iter()
            .zip(live)
            .filter_map(|(node, live)| live.then_some(node))
            .collect();
    }
    let used: HashSet<String> = graph
        .node
        .iter()
        .flat_map(|node| node.input.iter().cloned())
        .collect();
    let unused: HashSet<String> = graph
        .initializer
        .iter()
        .map(|t| t.name.clone())
        .filter(|name| !used.contains(name))
        .collect();
    graph.initializer.retain(|t| !unused.contains(&t.name));
    // An older model lists its initializers among the graph's inputs too.
    graph.input.retain(|input| !unused.contains(&input.name));
    let produced: HashSet<String> = graph
        .node
        .iter()
        .flat_map(|node| node.output.iter().cloned())
        .collect();
    graph.value_info.retain(|v| produced.contains(&v.name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tract_onnx::prelude::*;

    fn node(op_type: &str, input: &[&str], output: &[&str]) -> pb::NodeProto {
        pb::NodeProto {
            name: output[0].to_string(),
            op_type: op_type.to_string(),
            input: input.iter().map(ToString::to_string).collect(),
            output: output.iter().map(ToString::to_string).collect(),
            ..pb::NodeProto::default()
        }
    }

    fn tensor(name: &str, dims: &[i64], data_type: i32, raw_data: Vec<u8>) -> pb::TensorProto {
        pb::TensorProto {
            name: name.to_string(),
            dims: dims.to_vec(),
            data_type,
            raw_data,
            ..pb::TensorProto::default()
        }
    }

    fn value(name: &str) -> pb::ValueInfoProto {
        pb::ValueInfoProto {
            name: name.to_string(),
            ..pb::ValueInfoProto::default()
        }
    }

    /// The test model's input `x`: an `f32` tensor of 2 × 4.
    fn input() -> pb::ValueInfoProto {
        use pb::tensor_shape_proto::{Dimension, dimension};
        let dim = |n| Dimension {
            value: Some(dimension::Value::DimValue(n)),
            ..Dimension::default()
        };
        pb::ValueInfoProto {
            r#type: Some(pb::TypeProto {
                value: Some(pb::type_proto::Value::TensorType(pb::type_proto::Tensor {
                    elem_type: FLOAT,
                    shape: Some(pb::TensorShapeProto {
                        dim: vec![dim(2), dim(4)],
                    }),
                })),
                ..pb::TypeProto::default()
            }),
            ..value("x")
        }
    }

    /// The 4 × 3 weight matrix of the test model, as stored: `i8`, zero
    /// point 3, one scale per column.
    const W_Q: [i8; 12] = [12, -40, 7, 3, 90, -128, 127, 0, -5, -60, 33, 64];
    const W_ZP: i8 = 3;
    const W_SCALE: [f32; 3] = [0.02, 0.05, 0.01];

    /// `y = x · W`, as the quantizer exports it.
    fn quantized_model() -> pb::ModelProto {
        let mut cast = node("Cast", &["product"], &["product_f32"]);
        cast.attribute.push(pb::AttributeProto {
            name: "to".to_string(),
            i: i64::from(FLOAT),
            r#type: pb::attribute_proto::AttributeType::Int as i32,
            ..pb::AttributeProto::default()
        });
        pb::ModelProto {
            ir_version: 7,
            opset_import: vec![pb::OperatorSetIdProto {
                domain: String::new(),
                version: 13,
            }],
            graph: Some(pb::GraphProto {
                name: "test".to_string(),
                node: vec![
                    node("DynamicQuantizeLinear", &["x"], &["x_q", "x_scale", "x_zp"]),
                    node(
                        "MatMulInteger",
                        &["x_q", "w_q", "x_zp", "w_zp"],
                        &["product"],
                    ),
                    cast,
                    node("Mul", &["x_scale", "w_scale"], &["scales"]),
                    node("Mul", &["product_f32", "scales"], &["y"]),
                ],
                initializer: vec![
                    tensor("w_q", &[4, 3], INT8, W_Q.iter().map(|&q| q as u8).collect()),
                    tensor("w_zp", &[], INT8, vec![W_ZP as u8]),
                    tensor(
                        "w_scale",
                        &[3],
                        FLOAT,
                        W_SCALE.iter().flat_map(|s| s.to_le_bytes()).collect(),
                    ),
                ],
                input: vec![input()],
                output: vec![value("y")],
                ..pb::GraphProto::default()
            }),
            ..pb::ModelProto::default()
        }
    }

    const X: [f32; 8] = [0.5, -1.25, 2.0, 0.75, -0.3, 0.9, 1.6, -2.2];

    fn run(model: &pb::ModelProto) -> Vec<f32> {
        let plan = tract_onnx::onnx()
            .model_for_proto_model(model)
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .unwrap();
        let x: Tensor = tract_ndarray::Array2::from_shape_vec((2, 4), X.to_vec())
            .unwrap()
            .into();
        let y = plan.run(tvec!(x.into())).unwrap()[0].clone().into_tensor();
        y.to_plain_array_view::<f32>()
            .unwrap()
            .iter()
            .copied()
            .collect()
    }

    /// `x · W` in f32, with `W` dequantized.
    fn exact() -> Vec<f32> {
        let w = |r: usize, c: usize| (f32::from(W_Q[r * 3 + c]) - f32::from(W_ZP)) * W_SCALE[c];
        (0..2)
            .flat_map(|row| (0..3).map(move |c| (0..4).map(|r| X[row * 4 + r] * w(r, c)).sum()))
            .collect()
    }

    #[test]
    fn a_quantized_product_becomes_an_f32_one() {
        let original = quantized_model();
        let mut model = original.clone();
        assert_eq!(dequantize_matmuls(&mut model).unwrap(), (1, 1));

        let graph = model.graph.as_ref().unwrap();
        let ops: Vec<&str> = graph.node.iter().map(|n| n.op_type.as_str()).collect();
        assert_eq!(ops, ["MatMul"], "quantizer, cast and scales are gone");
        let names: Vec<&str> = graph.initializer.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["w_q/dequantized"]);

        // The rewritten product is the f32 one; the quantized product only
        // comes close, with its activations rounded to 8 bits.
        let (exact, rewritten, quantized) = (exact(), run(&model), run(&original));
        for ((e, r), q) in exact.iter().zip(&rewritten).zip(&quantized) {
            assert!((e - r).abs() < 1e-5, "{exact:?} against {rewritten:?}");
            assert!((e - q).abs() < 0.05, "{exact:?} against {quantized:?}");
        }
    }

    #[test]
    fn a_graph_without_quantized_products_is_left_as_it_is() {
        let mut model = quantized_model();
        let graph = model.graph.as_mut().unwrap();
        graph.node = vec![node("MatMul", &["x", "w"], &["y"])];
        graph.initializer = vec![tensor(
            "w",
            &[4, 3],
            FLOAT,
            [0.5_f32; 12].iter().flat_map(|w| w.to_le_bytes()).collect(),
        )];
        let before = model.clone();
        assert_eq!(dequantize_matmuls(&mut model).unwrap(), (0, 0));
        assert_eq!(model, before);
    }

    /// A product whose result is used twice is not the quantizer's chain:
    /// it stays quantized, and the model still runs.
    #[test]
    fn a_chain_of_another_shape_is_left_quantized() {
        let mut model = quantized_model();
        let graph = model.graph.as_mut().unwrap();
        graph.node.push(node("Identity", &["product"], &["also"]));
        graph.output.push(value("also"));
        let before = model.clone();
        assert_eq!(dequantize_matmuls(&mut model).unwrap(), (0, 1));
        assert_eq!(model, before);
        assert_eq!(run(&model).len(), 6);
    }
}
