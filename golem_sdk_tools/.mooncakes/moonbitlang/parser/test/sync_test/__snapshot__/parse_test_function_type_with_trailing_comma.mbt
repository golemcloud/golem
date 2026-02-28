type Callback1 (Bool,) -> Int
type Callback2 (Bool,Int,) -> Int 

pub(all) type ModalRenderer[T] (
  (@node.DispatchFn[T]) -> Unit raise @node.RespoCommonError,
) -> @node.RespoNode[T] raise @node.RespoCommonError
