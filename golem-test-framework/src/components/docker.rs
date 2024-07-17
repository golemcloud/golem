use testcontainers::{Container, Image};

pub trait KillContainer {
    fn kill(&self, keep: bool);
}

impl<'d, I: Image> KillContainer for Container<'d, I> {
    fn kill(&self, keep: bool) {
        if keep {
            self.stop();
        } else {
            self.rm();
        }
    }
}
