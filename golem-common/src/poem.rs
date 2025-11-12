use poem::endpoint::EitherEndpoint;
use poem::{IntoEndpoint, Middleware};

pub trait LazyEndpointExt: IntoEndpoint {
    fn with_if_lazy<T>(
        self,
        enable: bool,
        middleware: impl FnOnce() -> T,
    ) -> EitherEndpoint<Self, T::Output>
    where
        T: Middleware<Self::Endpoint>,
        Self: Sized,
    {
        if !enable {
            EitherEndpoint::A(self)
        } else {
            EitherEndpoint::B(middleware().transform(self.into_endpoint()))
        }
    }
}

impl<T: IntoEndpoint> LazyEndpointExt for T {}
