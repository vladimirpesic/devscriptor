//! The index and the language-server bridge tell an Objective-C `.m` from a
//! MATLAB one alike: each reads the same markers, in a crate of its own, and
//! a file the index parses as Objective-C is one clangd is asked about.
#![cfg(feature = "lsp")]

use devscriptor_core::parser::language_config::content_is_objc;
use devscriptor_lsp::languages::is_objective_c;

#[test]
fn the_index_and_the_bridge_tell_objective_c_alike() {
    let dir = tempfile::tempdir().unwrap();
    for (name, source, objective_c) in [
        ("Header.m", "#import <Foundation/Foundation.h>\n", true),
        ("Class.m", "@interface Limits : NSObject\n@end\n", true),
        ("Impl.m", "// limits\n@implementation Limits\n@end\n", true),
        ("plot.m", "function plot(x)\n  disp(x);\nend\n", false),
        ("script.m", "% #include is no Objective-C\nx = 1;\n", false),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, source).unwrap();
        assert_eq!(content_is_objc(source), objective_c, "{name}: index");
        assert_eq!(is_objective_c(&path), objective_c, "{name}: bridge");
    }
}
