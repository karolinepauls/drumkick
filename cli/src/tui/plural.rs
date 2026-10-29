pub fn naive_plural_suffix(length: usize) -> &'static str {
    if length == 1 { "" } else { "s" }
}
