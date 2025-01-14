use hyle::utils::conf::{Conf, Consensus};

use crate::find_available_port;

pub struct ConfMaker {
    i: u32,
    pub default: Conf,
}

impl ConfMaker {
    pub fn build(&mut self, prefix: &str) -> Conf {
        self.i += 1;
        Conf {
            id: if prefix == "single-node" {
                prefix.into()
            } else {
                format!("{}-{}", prefix, self.i)
            },
            host: format!("localhost:{}", find_available_port()),
            da_address: format!("localhost:{}", find_available_port()),
            tcp_server_address: Some(format!("localhost:{}", find_available_port())),
            rest: format!("localhost:{}", find_available_port()),
            ..self.default.clone()
        }
    }
}

impl Default for ConfMaker {
    fn default() -> Self {
        let mut default = Conf::new(None, None, None).unwrap();
        default.single_node = Some(false);
        default.run_indexer = false; // disable indexer by default to avoid needed PG
        default.log_format = "node".to_string(); // Activate node name in logs for convenience in tests.
        default.consensus = Consensus {
            slot_duration: 1,
            genesis_stakers: {
                let mut stakers = std::collections::HashMap::new();
                stakers.insert("node-1".to_owned(), 100);
                stakers.insert("node-2".to_owned(), 100);
                stakers
            },
        };
        Self { i: 0, default }
    }
}
