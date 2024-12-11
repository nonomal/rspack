use std::collections::HashMap;

use rspack_core::{Compilation, CompilationOptimizeModules, CompilerOptions, Module, Plugin};
use rspack_error::{Error, Result};
use rspack_hook::{plugin, plugin_hook};
use rspack_regex::RspackRegex;
use rspack_util::path::relative;

// #[derive(Debug)]
pub struct CircularDependencyPluginOptions {
  // Constant overhead for a chunk.
  pub exclude: Option<RspackRegex>,
  pub include: Option<RspackRegex>,
  pub fail_on_error: bool,
  pub allow_async_cycles: bool,
  pub on_detected: Option<Box<dyn Fn(&dyn Module, Vec<String>, &Compilation) -> Result<()>>>,
  pub cwd: String,
}

#[plugin]
// #[derive(Debug)]
pub struct CircularDependencyPlugin {
  options: CircularDependencyPluginOptions,
}

impl CircularDependencyPlugin {
  pub fn new(options: CircularDependencyPluginOptions) -> Self {
    let merged_options = CircularDependencyPluginOptions {
      exclude: RspackRegex::new("$^").ok(),
      include: RspackRegex::new(".*").ok(),
      fail_on_error: false,
      allow_async_cycles: false,
      on_detected: None,
      cwd: std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string(),
      ..options
    };

    Self::new_inner(merged_options)
  }

  fn is_cyclic(
    &self,
    initial_module: Box<&dyn Module>,
    current_module: Box<&dyn Module>,
    seen_modules: &mut HashMap<u32, bool>,
    compilation: &Compilation,
  ) -> Option<Vec<String>> {
    let cwd = self.options.cwd.to_string();

    let dependencies = initial_module.dependencies;

    // Add the current module to the seen modules cache
    seen_modules.insert(current_module.debug_id, true);

    // If the modules aren't associated to resources
    // it's not possible to display how they are cyclical
    if current_module.original_source().is_none() || initial_module.original_source().is_none() {
      return None;
    }

    // Iterate over the current modules dependencies
    for dependency in current_module.get_dependencies() {
      if let Some(dep_module) = if let Some(module_graph) = &compilation.module_graph {
        // handle getting a module for webpack 5
        module_graph.get_module(dependency)
      } else {
        // handle getting a module for webpack 4
        dependency
      } {
        // ignore dependencies that don't have an associated resource
        if dep_module.resource.is_none() {
          continue;
        }
        // ignore dependencies that are resolved asynchronously
        if self.options.allow_async_cycles.unwrap_or(false) && dependency.weak {
          continue;
        }
        // the dependency was resolved to the current module due to how webpack internals
        // setup dependencies like CommonJsSelfReferenceDependency and ModuleDecoratorDependency
        if current_module == dep_module {
          continue;
        }

        if seen_modules.contains_key(&dep_module.debug_id) {
          if dep_module.debug_id == initial_module.debug_id {
            // Initial module has a circular dependency
            let current_resource = current_module.resource.as_ref().unwrap();
            let dep_resource = dep_module.resource.as_ref().unwrap();
            return Some(vec![
              relative(&cwd, current_resource).to_string_lossy().into(),
              relative(&cwd, dep_resource).to_string_lossy().into(),
            ]);
          }
          // Found a cycle, but not for this module
          continue;
        }

        if let Some(mut maybe_cyclical_paths_list) =
          self.is_cyclic(initial_module, dep_module, seen_modules, compilation)
        {
          maybe_cyclical_paths_list.insert(
            0,
            relative(&cwd, &current_module.resource.unwrap())
              .to_string_lossy()
              .into(),
          );
          return Some(maybe_cyclical_paths_list);
        }
      }
    }

    None
  }
}

#[plugin_hook(CompilationOptimizeModules for CircularDependencyPlugin)]
fn optimize_modules(&self, compilation: &mut Compilation) -> Result<Option<bool>> {
  let plugin = &self;

  // if plugin.options.on_start {
  //   plugin.options.on_start(compilation);
  // }
  for module in compilation.modules.values() {
    let should_skip = (module.resource.is_none()
      || plugin.options.exclude.test(module.resource)
      || !plugin.options.include.test(module.resource));
    // skip the module if it matches the exclude pattern
    if should_skip {
      continue;
    }

    let maybe_cyclical_paths_list = &self.is_cyclic(module, module, {}, compilation);
    if let Some(cyclical_paths_list) = maybe_cyclical_paths_list {
      // allow consumers to override all behavior with onDetected
      if let Some(on_detected) = &plugin.options.onDetected {
        match on_detected(Module {
          module,
          paths: maybe_cyclical_paths_list,
          compilation,
        }) {
          Ok(_) => {}
          Err(err) => {
            compilation.errors.push(err);
          }
        }
        continue;
      }

      // mark warnings or errors on webpack compilation
      let error = Err(Error::new(maybe_cyclical_paths_list.join(" -> ")));
      if plugin.options.failOnError {
        compilation.errors.push(error);
      } else {
        compilation.warnings.push(error);
      }
    }
  }
  // if plugin.options.onEnd {
  //   plugin.options.onEnd(compilation);
  // }

  Ok(None)
}

impl Plugin for CircularDependencyPlugin {
  fn name(&self) -> &'static str {
    "CircularDependencyPlugin"
  }

  fn apply(
    &self,
    ctx: rspack_core::PluginContext<&mut rspack_core::ApplyContext>,
    _options: &CompilerOptions,
  ) -> Result<()> {
    ctx
      .context
      .compilation_hooks
      .optimize_modules
      .tap(optimize_modules::new(self));
    Ok(())
  }

  fn clear_cache(&self) {}
}
