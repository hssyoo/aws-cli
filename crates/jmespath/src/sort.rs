//! `list.sort()` by `<` alone, as CPython 3.14 sorts: stable, and for
//! fewer than 64 items its exact moves (`count_run`, then binary
//! insertion), so even an inconsistent `<` (a `NaN` among numbers) orders
//! them as Python does. Longer lists merge-sort, which agrees with Python
//! whenever `<` is a consistent order.

/// Sorts `items` by `lt`, CPython's way.
pub(crate) fn sort<T>(items: &mut Vec<T>, lt: impl Fn(&T, &T) -> bool) {
    if items.len() < 64 {
        let run = if items.is_empty() {
            0
        } else {
            count_run(items, &lt)
        };
        binary_sort(items, run, &lt);
    } else {
        let sorted = merge_sort(std::mem::take(items), &lt);
        *items = sorted;
    }
}

/// `count_run`: the length of the run at the start, made ascending.
fn count_run<T>(a: &mut [T], lt: &impl Fn(&T, &T) -> bool) -> usize {
    let len = a.len();
    let mut n = 1;
    while n < len && !lt(&a[n], &a[n - 1]) {
        n += 1;
    }
    if n == len {
        return n;
    }
    if n > 1 {
        if lt(&a[0], &a[n - 1]) {
            return n;
        }
        a[..n].reverse();
    }
    n += 1;
    let mut equal = 0;
    while n < len {
        if lt(&a[n], &a[n - 1]) {
            reverse_last(a, n, &mut equal);
        } else if lt(&a[n - 1], &a[n]) {
            break;
        } else {
            equal += 1;
        }
        n += 1;
    }
    reverse_last(a, n, &mut equal);
    a[..n].reverse();
    while n < len && !lt(&a[n], &a[n - 1]) {
        n += 1;
    }
    n
}

/// `REVERSE_LAST_NEQ`: reverses the latest run of equal items in place.
fn reverse_last<T>(a: &mut [T], n: usize, equal: &mut usize) {
    if *equal > 0 {
        a[n - *equal - 1..n].reverse();
        *equal = 0;
    }
}

/// `binarysort`: binary insertion of `a[sorted..]` into the sorted prefix.
fn binary_sort<T>(a: &mut [T], sorted: usize, lt: &impl Fn(&T, &T) -> bool) {
    for ok in sorted.max(1)..a.len() {
        let (mut left, mut right) = (0, ok);
        while left < right {
            let middle = (left + right) >> 1;
            if lt(&a[ok], &a[middle]) {
                right = middle;
            } else {
                left = middle + 1;
            }
        }
        a[left..=ok].rotate_right(1);
    }
}

fn merge_sort<T>(mut items: Vec<T>, lt: &impl Fn(&T, &T) -> bool) -> Vec<T> {
    if items.len() < 2 {
        return items;
    }
    let right = merge_sort(items.split_off(items.len() / 2), lt);
    let left = merge_sort(items, lt);
    let mut merged = Vec::with_capacity(left.len() + right.len());
    let (mut left, mut right) = (left.into_iter().peekable(), right.into_iter().peekable());
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        let next = if lt(b, a) { right.next() } else { left.next() };
        merged.extend(next);
    }
    merged.extend(left);
    merged.extend(right);
    merged
}

#[cfg(test)]
mod tests;
