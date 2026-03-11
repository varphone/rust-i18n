# An example of sharing I18n translations between crates.

This example shows the current recommended setup for a workspace: expand `rust_i18n::i18n!` once in the shared `i18n` crate, then let the other crates call `t!` without running their own `i18n!` initialization.

That keeps the translation data embedded only once, reduces duplicate codegen, and lets every linked crate reuse the same global provider.

本示例展示了当前推荐的工作区用法：只在共享的 `i18n` crate 中执行一次 `rust_i18n::i18n!`，其他 crate 直接调用 `t!`，不再各自执行初始化。

这样可以让翻译数据只嵌入一次，减少重复代码生成，并让所有已链接的 crate 共享同一个全局 provider。

- [i18n](crate/i18n) - Registers the shared provider once and re-exports the public helpers.
- [my-app1](crate/my-app1) - Uses translations from the shared `i18n` crate without any extra init step.
- [my-app2](crate/my-app2) - Another crate that just imports `t!` from `i18n` and uses the shared provider.
