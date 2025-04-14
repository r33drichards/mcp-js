https://github.com/denoland/rusty_v8/blob/main/tests/test_api.rs#L5352

```rust

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


#[test]
fn snapshot_creator() {
  let _setup_guard = setup::sequential_test();
  // First we create the snapshot, there is a single global variable 'a' set to
  // the value 3.
  let isolate_data_index;
  let context_data_index;
  let context_data_index_2;
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

  let startup_data = {
    let mut snapshot_creator =
      v8::Isolate::snapshot_creator_from_existing_snapshot(
        startup_data,
        None,
        None,
      );
    {
      // Check that the SnapshotCreator isolate has been set up correctly.
      let _ = snapshot_creator.thread_safe_handle();

      let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
      let context = v8::Context::new(scope, Default::default());
      let scope = &mut v8::ContextScope::new(scope, context);
      eval(scope, "a = 1 + 2").unwrap();

      scope.set_default_context(context);

      let n1 = v8::Number::new(scope, 1.0);
      let n2 = v8::Number::new(scope, 2.0);
      let n3 = v8::Number::new(scope, 3.0);
      isolate_data_index = scope.add_isolate_data(n1);
      context_data_index = scope.add_context_data(context, n2);
      context_data_index_2 = scope.add_context_data(context, n3);
    }
    snapshot_creator
      .create_blob(v8::FunctionCodeHandling::Clear)
      .unwrap()
  };
  assert!(!startup_data.is_empty());
  // Now we try to load up the snapshot and check that 'a' has the correct
  // value.
  {
    let params = v8::Isolate::create_params().snapshot_blob(startup_data);
    let isolate = &mut v8::Isolate::new(params);
    {
      let scope = &mut v8::HandleScope::new(isolate);
      let context = v8::Context::new(scope, Default::default());
      let scope = &mut v8::ContextScope::new(scope, context);
      let result = eval(scope, "a === 3").unwrap();
      let true_val = v8::Boolean::new(scope, true).into();
      assert!(result.same_value(true_val));

      let result = eval(scope, "b === 5").unwrap();
      let true_val = v8::Boolean::new(scope, true).into();
      assert!(result.same_value(true_val));

      let isolate_data = scope
        .get_isolate_data_from_snapshot_once::<v8::Value>(isolate_data_index);
      assert!(isolate_data.unwrap() == v8::Number::new(scope, 1.0));
      let no_data_err = scope
        .get_isolate_data_from_snapshot_once::<v8::Value>(isolate_data_index);
      assert!(matches!(no_data_err, Err(v8::DataError::NoData { .. })));

      let context_data = scope
        .get_context_data_from_snapshot_once::<v8::Value>(context_data_index);
      assert!(context_data.unwrap() == v8::Number::new(scope, 2.0));
      let no_data_err = scope
        .get_context_data_from_snapshot_once::<v8::Value>(context_data_index);
      assert!(matches!(no_data_err, Err(v8::DataError::NoData { .. })));

      let bad_type_err = scope
        .get_context_data_from_snapshot_once::<v8::Private>(
          context_data_index_2,
        );
      assert!(matches!(bad_type_err, Err(v8::DataError::BadType { .. })));
      // Ensure we can compile a request for v8::Data
      _ = scope
        .get_context_data_from_snapshot_once::<v8::Data>(context_data_index_2);
    }
  }
}
```