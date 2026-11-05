use rb_app::startup;

fn main() {
    let selector = startup::connection_selector(
        std::env::args().skip(1),
        std::env::var("RB_CONNECTION").ok(),
    );
    let startup = startup::load_default(selector.as_deref());
    // The runtime lives in main's scope until `run` returns.
    let rt = rb_app::app::build_runtime();
    rb_app::app::run(startup, rt.handle().clone());
}
