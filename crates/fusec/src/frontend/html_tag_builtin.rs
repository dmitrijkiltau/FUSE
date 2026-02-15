use crate::html_tags;

pub fn should_use_html_tag_builtin(
    name: &str,
    shadowed_by_value: bool,
    shadowed_by_function: bool,
    shadowed_by_config: bool,
    shadowed_by_import: bool,
) -> bool {
    if html_tags::tag_kind(name).is_none() {
        return false;
    }
    !(shadowed_by_value || shadowed_by_function || shadowed_by_config || shadowed_by_import)
}
