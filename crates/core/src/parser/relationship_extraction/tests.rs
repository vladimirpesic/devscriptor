use super::*;

/// The innermost holder as a walk over every extent before the place
/// finds it, which [`Extents::innermost`] must agree with.
fn innermost_by_walking(
    sorted: &[Extent],
    from: Position,
    holds: impl Fn(&Extent) -> bool,
) -> Option<Uuid> {
    let candidates = &sorted[..sorted.partition_point(|(start, _, _)| *start <= from)];
    let mut innermost: Option<&Extent> = None;
    for extent in candidates.iter().rev() {
        if innermost.is_some_and(|(start, _, _)| extent.0 < *start) {
            break;
        }
        if holds(extent) && innermost.is_none_or(|(_, end, _)| extent.1 < *end) {
            innermost = Some(extent);
        }
    }
    innermost.map(|(_, _, id)| *id)
}

/// Nested, side by side, sharing a start or an end (a Python class ends
/// where its last method does), or overlapping as no nodes do: the
/// innermost holder of every place and of every extent is the one a walk
/// over all of them finds.
#[test]
fn the_innermost_holder_is_the_one_a_full_walk_finds() {
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |bound: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        u32::try_from(seed % u64::from(bound)).unwrap()
    };
    for _ in 0..500 {
        let count = next(30) as usize;
        let mut extents: Vec<Extent> = (0..count)
            .map(|_| {
                let start = (next(12), next(4));
                let end = (start.0 + next(6), next(4));
                (start, end.max(start), Uuid::new_v4())
            })
            .collect();
        extents.sort_by_key(|(start, _, _)| *start);
        let index = Extents::new_sorted(extents);
        for line in 0..20 {
            for column in 0..5 {
                let at = (line, column);
                let holds = |(start, end, _): &Extent| *start <= at && at < *end;
                assert_eq!(
                    index.innermost(at, at, holds),
                    innermost_by_walking(&index.sorted, at, holds),
                    "{at:?} in {:?}",
                    index.sorted
                );
            }
        }
        for &(start, end, _) in &index.sorted {
            let holds = |(c_start, c_end, _): &Extent| {
                *c_start <= start && end <= *c_end && (*c_start, *c_end) != (start, end)
            };
            assert_eq!(
                index.innermost(start, end, holds),
                innermost_by_walking(&index.sorted, start, holds),
                "{start:?}..{end:?} in {:?}",
                index.sorted
            );
        }
    }
}

/// A forward kind and its reverse sit on opposite sides of `is_reverse`,
/// which tells a file's own edges from the edges other files have into it
/// when the edges of a re-parsed file are replaced.
#[test]
fn every_reverse_pair_is_classified_by_is_reverse() {
    let mut pairs = 0;
    for kind in RelationshipType::ALL {
        if let Some(reverse) = reverse_of(&kind) {
            assert!(!kind.is_reverse(), "{kind:?} is a forward kind");
            assert!(
                reverse.is_reverse(),
                "{reverse:?} is the reverse of {kind:?}"
            );
            pairs += 1;
        }
    }
    assert_eq!(pairs, 6, "every reverse kind has its forward kind");
    assert_eq!(
        RelationshipType::ALL
            .iter()
            .filter(|k| k.is_reverse())
            .count(),
        pairs
    );
    // HasMethod and MethodOf are created as a pair from one file's
    // nesting; both are that file's own edges.
    assert!(!RelationshipType::HasMethod.is_reverse());
    assert!(!RelationshipType::MethodOf.is_reverse());
}

/// `ALL` lists each kind once, and each round-trips through its string.
#[test]
fn all_lists_each_kind_once() {
    let unique: std::collections::HashSet<_> = RelationshipType::ALL.iter().collect();
    assert_eq!(unique.len(), RelationshipType::ALL.len());
    for kind in RelationshipType::ALL {
        assert_eq!(RelationshipType::parse(kind.as_str()).unwrap(), kind);
    }
}
