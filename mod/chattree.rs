// The chat box's command tree: /wp gets its words (add, del, ...) to complete.
fn official_worldmap_chat_tree(value: horizon_client::command::Dispatcher) -> horizon_client::command::Dispatcher {
    crate::game::official_worldmap::extend_tree(value)
}
