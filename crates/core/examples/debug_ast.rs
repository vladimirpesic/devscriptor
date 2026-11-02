//! Debug script to dump AST node types for a given file

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cargo run --example debug_ast -- <file_path>");
        std::process::exit(1);
    }

    let file_path = Path::new(&args[1]);
    let content = std::fs::read_to_string(file_path)?;

    // Detect language from extension
    let extension = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let language = match extension {
        "fs" => "fsharp",
        "m" => "objc",
        "cmake" => "cmake",
        "html" => "html",
        "css" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "scss" => "scss",
        "xml" => "xml",
        _ => extension,
    };

    println!("Language: {language}");

    // Get the language from loader
    let lang = devscriptor_core::parser::language_loader::get_language(language)?;

    // Parse the file
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang)?;

    let tree = parser.parse(&content, None).expect("Failed to parse");

    // Collect all unique node types
    let mut node_types = std::collections::HashSet::new();
    let mut function_nodes = Vec::new();
    let mut class_nodes = Vec::new();

    fn traverse_tree(
        node: tree_sitter::Node,
        content: &str,
        node_types: &mut std::collections::HashSet<String>,
        function_nodes: &mut Vec<String>,
        class_nodes: &mut Vec<String>,
        depth: usize,
    ) {
        let node_type = node.kind();
        node_types.insert(node_type.to_string());

        // Check if it might be a function or class using tree-sitter node kinds
        let node_text = &content[node.start_byte()..node.end_byte().min(node.start_byte() + 80)];
        let is_function = matches!(
            node_type,
            "function_declaration"
                | "function_definition"
                | "method_definition"
                | "arrow_function"
                | "def"
                | "function_item"
                | "closure_expression"
                | "lambda"
                | "method_declaration"
        );
        let is_class = matches!(
            node_type,
            "class_declaration"
                | "class_definition"
                | "struct_item"
                | "struct_specifier"
                | "interface_declaration"
                | "enum_declaration"
                | "type_declaration"
                | "impl_item"
        );

        if is_function {
            function_nodes.push(format!(
                "{}: {}",
                node_type,
                node_text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('{')
                    .trim()
            ));
        }
        if is_class {
            class_nodes.push(format!(
                "{}: {}",
                node_type,
                node_text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('{')
                    .trim()
            ));
        }

        let indent = "  ".repeat(depth);
        if depth < 3 {
            println!(
                "{}{} [{}:{}-{}:{}]",
                indent,
                node_type,
                node.start_position().row + 1,
                node.start_position().column,
                node.end_position().row + 1,
                node.end_position().column
            );
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                traverse_tree(
                    child,
                    content,
                    node_types,
                    function_nodes,
                    class_nodes,
                    depth + 1,
                );
            }
        }
    }

    traverse_tree(
        tree.root_node(),
        &content,
        &mut node_types,
        &mut function_nodes,
        &mut class_nodes,
        0,
    );

    println!("\n=== All Node Types ===");
    let mut sorted_types: Vec<_> = node_types.into_iter().collect();
    sorted_types.sort();
    for t in sorted_types {
        println!("  - {t}");
    }

    println!("\n=== Potential Function Nodes ===");
    for f in function_nodes {
        println!("  {f}");
    }

    println!("\n=== Potential Class/Type Nodes ===");
    for c in class_nodes {
        println!("  {c}");
    }

    Ok(())
}
