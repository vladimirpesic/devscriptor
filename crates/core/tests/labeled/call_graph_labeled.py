# Labeled call graph fixture for the call-edge precision/recall measurement
# (test_call_graph_precision.rs).
#
# Expected call edges are in call_graph_labeled.json.
# Noise sources to test false-positive suppression:
#   - String literal: "leaf_alpha() is mentioned but not called here"
#   - Comment: # leaf_beta() is not called in leaf_alpha

DESCRIPTION = "This module calls leaf_alpha and leaf_beta in various ways."  # string, not a call


def leaf_alpha():
    """Returns alpha value. Does not call leaf_beta()."""  # docstring, not a call
    return 1


def leaf_beta():
    # leaf_alpha() is mentioned in this comment but not called
    return 2


def leaf_gamma():
    return 3


def mid_a():
    x = leaf_alpha()
    return x + 1


def mid_b():
    x = leaf_alpha()
    y = leaf_beta()
    return x + y


def mid_c():
    result = leaf_gamma()
    extra = leaf_alpha()
    return result + extra


def top_caller():
    a = mid_a()
    b = mid_b()
    c = mid_c()
    return a + b + c


def unrelated():
    # Makes no calls to project-defined functions.
    return 42
