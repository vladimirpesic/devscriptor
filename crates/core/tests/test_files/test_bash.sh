#!/bin/bash
# Comprehensive Bash test file containing all entities and relationships.

# Global variables
readonly SCRIPT_NAME="$(basename "$0")"
readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="1.0.0"
DEBUG=false

# Associative arrays (hash maps)
declare -A CONFIG
declare -a LOG_LEVELS=(DEBUG INFO WARN ERROR)

# Configuration
CONFIG[host]="localhost"
CONFIG[port]=8080
CONFIG[timeout]=30

# Function definitions

# Simple function
greet() {
    echo "Hello, World!"
}

# Function with parameters
print_message() {
    local level="$1"
    local message="$2"
    echo "[$level] $message"
}

# Function with return value
add() {
    local a=$1
    local b=$2
    echo $((a + b))
}

# Function with local variables
process_data() {
    local input_file="$1"
    local output_file="$2"
    local temp_dir=$(mktemp -d)

    if [[ ! -f "$input_file" ]]; then
        echo "Error: Input file not found" >&2
        return 1
    fi

    # Process data
    cp "$input_file" "$temp_dir/processed"
    mv "$temp_dir/processed" "$output_file"
    rm -rf "$temp_dir"

    return 0
}

# Function with command substitution
get_date() {
    local format="${1:-%Y-%m-%d}"
    date +"$format"
}

# Function with pipeline
count_lines() {
    local file="$1"
    wc -l < "$file" 2>/dev/null || echo "0"
}

# Recursive function
factorial() {
    local n=$1
    if [[ $n -le 1 ]]; then
        echo 1
    else
        echo $((n * $(factorial $((n - 1)))))
    fi
}

# Function with array handling
sum_array() {
    local -n arr=$1
    local sum=0
    for val in "${arr[@]}"; do
        sum=$((sum + val))
    done
    echo $sum
}

# Function with options/flags
parse_options() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            -v|--verbose)
                DEBUG=true
                shift
                ;;
            -h|--help)
                show_help
                exit 0
                ;;
            -o|--output)
                OUTPUT_FILE="$2"
                shift 2
                ;;
            *)
                echo "Unknown option: $1" >&2
                exit 1
                ;;
        esac
    done
}

# Help function
show_help() {
    cat << EOF
Usage: $SCRIPT_NAME [OPTIONS]

Options:
    -h, --help      Show this help message
    -v, --verbose   Enable verbose output
    -o, --output    Specify output file

Examples:
    $SCRIPT_NAME -v
    $SCRIPT_NAME --output result.txt
EOF
}

# Error handling function
die() {
    local message="$1"
    local code="${2:-1}"
    echo "Error: $message" >&2
    exit "$code"
}

# Logging function
log() {
    local level="$1"
    shift
    local message="$*"
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')

    if [[ "$DEBUG" == true || "$level" != "DEBUG" ]]; then
        echo "[$timestamp] [$level] $message"
    fi
}

# Validation function
validate_integer() {
    local value="$1"
    local var_name="$2"

    if ! [[ "$value" =~ ^-?[0-9]+$ ]]; then
        die "$var_name must be an integer, got: $value"
    fi
}

# File operation function
backup_file() {
    local file="$1"
    local backup_dir="${2:-./backups}"

    if [[ ! -f "$file" ]]; then
        die "File not found: $file"
    fi

    mkdir -p "$backup_dir"
    local timestamp=$(date +%Y%m%d_%H%M%S)
    local filename=$(basename "$file")
    local backup_name="${filename}.${timestamp}.bak"

    cp "$file" "$backup_dir/$backup_name"
    log INFO "Created backup: $backup_dir/$backup_name"
}

# String manipulation function
string_reverse() {
    local str="$1"
    echo "$str" | rev
}

# Subshell function
run_in_subshell() {
    (
        local var="subshell_value"
        echo "Inside subshell: $var"
        cd /tmp || exit
        pwd
    )
    echo "Outside subshell, var is unset"
}

# Trap handling
setup_traps() {
    trap 'echo "Interrupted!"; exit 130' INT
    trap 'echo "Terminated!"; exit 143' TERM
    trap cleanup EXIT
}

# Cleanup function
cleanup() {
    log INFO "Cleaning up temporary resources..."
    [[ -n "$TEMP_DIR" ]] && rm -rf "$TEMP_DIR"
}

# Conditional logic example
check_system() {
    local os=$(uname -s)
    local arch=$(uname -m)

    case "$os" in
        Linux)
            log INFO "Running on Linux ($arch)"
            ;;
        *)
            log WARN "Unknown OS: $os"
            ;;
    esac
}

# Loop examples
demonstrate_loops() {
    # For loop with range
    log INFO "For loop with range:"
    for i in {1..5}; do
        echo "Iteration $i"
    done

    # For loop with array
    log INFO "For loop with array:"
    local fruits=("apple" "banana" "cherry")
    for fruit in "${fruits[@]}"; do
        echo "Fruit: $fruit"
    done

    # While loop
    log INFO "While loop:"
    local count=0
    while [[ $count -lt 3 ]]; do
        echo "Count: $count"
        ((count++))
    done

    # Until loop
    log INFO "Until loop:"
    local n=0
    until [[ $n -ge 3 ]]; do
        echo "N: $n"
        ((n++))
    done

    # Select loop (menu)
    log INFO "Select construct:"
    local options=("Option 1" "Option 2" "Option 3" "Quit")
    select opt in "${options[@]}"; do
        case $opt in
            "Option 1") echo "Selected 1"; break ;;
            "Option 2") echo "Selected 2"; break ;;
            "Option 3") echo "Selected 3"; break ;;
            "Quit") break ;;
            *) echo "Invalid option" ;;
        esac
    done
}

# Here-document and here-string examples
use_heredocs() {
    # Here-document
    cat << 'EOF'
This is a multi-line
heredoc string with
variable preservation
EOF

    # Here-string
    read -r var <<< "hello world"
    echo "Read: $var"
}

# Process substitution
use_process_subst() {
    local result
    while read -r line; do
        echo "Line: $line"
    done < <(echo -e "line1\nline2\nline3")
}

# Arithmetic operations
math_demo() {
    local a=10
    local b=3

    echo "Addition: $((a + b))"
    echo "Subtraction: $((a - b))"
    echo "Multiplication: $((a * b))"
    echo "Division: $((a / b))"
    echo "Modulo: $((a % b))"
    echo "Power: $((a ** b))"

    # Floating point with bc
    echo "Float division: $(echo "scale=2; $a / $b" | bc)"
}

# Main execution
main() {
    log INFO "Starting $SCRIPT_NAME v$VERSION"

    setup_traps
    check_system

    # Parse command line options
    parse_options "$@"

    # Demonstrate functions
    greet
    print_message "INFO" "Test message"

    local result
    result=$(add 5 3)
    log INFO "5 + 3 = $result"

    # Array operations
    local numbers=(1 2 3 4 5)
    local total
    total=$(sum_array numbers)
    log INFO "Sum of array: $total"

    # Loops demonstration
    demonstrate_loops

    # Math operations
    math_demo

    log INFO "Script completed successfully"
}

# Run main if executed directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
