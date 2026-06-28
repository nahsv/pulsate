;; Sample Pulsate plugin (ABI v1): eval doubles its input and logs the result.
(module
  (import "p8" "log" (func $log (param i32)))
  (func (export "pulsate_abi_version") (result i32) (i32.const 1))
  (func (export "eval") (param i32) (result i32)
    (call $log (i32.mul (local.get 0) (i32.const 2)))
    (i32.mul (local.get 0) (i32.const 2))))
