use log::info;

use std::io::Write;
use v8::OwnedIsolate;


fn eval<'s>(
    scope: &mut v8::HandleScope<'s>,
    code: &str,
  ) -> Option<v8::Local<'s, v8::Value>> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).unwrap();
    let script = v8::Script::compile(scope, source, None).unwrap();
    let r = script.run(scope);
    r.map(|v| scope.escape(v))
  }

fn main() {
    let startup_data = {
        let mut snapshot_creator = v8::Isolate::snapshot_creator(None, None);
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            eval(scope, "b = 2 + 3").unwrap();

            scope.set_default_context(context);
        }

        snapshot_creator
            .create_blob(v8::FunctionCodeHandling::Clear)
            .unwrap()
    };
    info!("snapshot created");
    info!("writing snapshot to file snapshot.bin in current directory");
    let mut file = std::fs::File::create("snapshot.bin").unwrap();
    file.write_all(&startup_data).unwrap();

}
