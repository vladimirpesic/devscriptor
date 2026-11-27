//! Whether code is test code: what a test runner calls, not the code it
//! tests. Dead-code analysis takes test code for reached, and a change's
//! impact lists the tests that reach what it changed.

use std::path::Path;

/// The names of directories whose files are tests.
const TEST_DIRECTORIES: &[&str] = &[
    "test",
    "tests",
    "spec",
    "specs",
    "__tests__",
    "testing",
    "Tests",
];

/// The decorators, annotations and attributes that mark a test for its runner,
/// by the last segment of their name, lower case: `#[test]`,
/// `#[tokio::test]`, `#[rstest]`, JUnit's `@Test` and `@ParameterizedTest`,
/// xUnit's `[Fact]` and `[Theory]`, MSTest's `[TestMethod]`, `NUnit`'s
/// `[TestCase]`.
const TEST_DECORATORS: &[&str] = &[
    "test",
    "rstest",
    "test_case",
    "quickcheck",
    "parameterizedtest",
    "repeatedtest",
    "testfactory",
    "testtemplate",
    "fact",
    "theory",
    "testmethod",
    "testcase",
    "testcasesource",
];

/// Whether the file at `path_in_project` (its path within its project, so
/// that a `test` folder above the project counts for nothing) holds tests: it
/// lies in a test directory, or is named as a test file (`test_cart.py`,
/// `cart_test.go`, `cart.spec.ts`, `CartTest.java`).
#[must_use]
pub fn is_test_path(path_in_project: &Path) -> bool {
    let in_test_dir = path_in_project.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|part| TEST_DIRECTORIES.contains(&part))
    });
    let stem = path_in_project
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let test_file = stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
        || stem.ends_with("Test")
        || stem.ends_with("Tests");
    in_test_dir || test_file
}

/// Whether an entity of `language` named `name` is named as a test:
/// `test_total`, `testTotal`, and in Go `TestTotal`, `BenchmarkTotal`,
/// `ExampleTotal`, `FuzzTotal`.
#[must_use]
pub fn names_a_test_in(name: &str, language: &str) -> bool {
    names_a_test(name, "test")
        || (language == "go"
            && ["Test", "Benchmark", "Example", "Fuzz"]
                .iter()
                .any(|prefix| names_a_test(name, prefix)))
}

/// Whether an entity named `name`, of `language`, in the file at
/// `path_in_project`, is test code by where it is or what it is called.
#[must_use]
pub fn is_test_code(path_in_project: &Path, name: &str, language: &str) -> bool {
    is_test_path(path_in_project) || names_a_test_in(name, language)
}

/// Whether `decorator` marks a test for its runner ([`TEST_DECORATORS`]).
#[must_use]
pub fn is_test_decorator(decorator: &str) -> bool {
    let last = decorator
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(decorator)
        .to_ascii_lowercase();
    TEST_DECORATORS.contains(&last.as_str())
}

/// Whether `name` is `prefix`, or `prefix` and then a word of its own — `test`,
/// `test_total`, `testTotal`, `TestTotal`, `Test1` — not a word beginning with
/// its letters (`testimonials`, `Testify`).
fn names_a_test(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_and_directories() {
        for path in [
            "tests/cart.rs",
            "src/__tests__/cart.js",
            "test_cart.py",
            "cart_test.go",
            "cart.spec.ts",
            "src/CartTest.java",
            "Tests/CartTests.swift",
        ] {
            assert!(is_test_path(Path::new(path)), "{path}");
        }
        for path in ["src/cart.rs", "src/testimonials.rs", "contest/cart.py"] {
            assert!(!is_test_path(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn test_names() {
        assert!(names_a_test_in("test_total", "python"));
        assert!(names_a_test_in("testTotal", "javascript"));
        assert!(names_a_test_in("TestTotal", "go"));
        assert!(names_a_test_in("BenchmarkTotal", "go"));
        assert!(!names_a_test_in("TestTotal", "java"));
        assert!(!names_a_test_in("testimonials", "python"));
        assert!(!names_a_test_in("Testify", "go"));
    }

    #[test]
    fn test_decorators() {
        for decorator in [
            "test",
            "tokio::test",
            "async_std::test",
            "Test",
            "ParameterizedTest",
            "org.junit.jupiter.api.Test",
            "Fact",
            "TestMethod",
        ] {
            assert!(is_test_decorator(decorator), "{decorator}");
        }
        for decorator in ["derive", "app.route", "pytest.fixture", "Testing"] {
            assert!(!is_test_decorator(decorator), "{decorator}");
        }
    }
}
