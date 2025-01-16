use std::{
    any::{type_name, TypeId},
    collections::HashMap,
};

use opentelemetry::{InstrumentationScope, KeyValue};
use quote::ToTokens;
use syn::{parse_str, Type};

#[derive(Debug, Clone)]
pub struct BusMetrics {
    labels: HashMap<(TypeId, TypeId), [KeyValue; 2]>,
    send: opentelemetry::metrics::Counter<u64>,
    receive: opentelemetry::metrics::Counter<u64>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl BusMetrics {
    pub fn global(meter_id: String) -> BusMetrics {
        let scope = InstrumentationScope::builder(meter_id).build();
        let my_meter = opentelemetry::global::meter_with_scope(scope);

        BusMetrics {
            labels: HashMap::new(),
            send: my_meter.u64_counter("send").build(),
            receive: my_meter.u64_counter("receive").build(),
        }
    }

    // Fonction pour simplifier le nom de type en utilisant `syn`
    fn simplify_type_name(type_name: &str) -> String {
        // Tente de parser `type_name` en tant que Type
        let parsed_type: Type = parse_str(type_name).expect("Erreur lors du parsing du type");

        // Fonction auxiliaire pour extraire les segments de base sans le chemin complet
        fn simplify_type(ty: &Type) -> String {
            match ty {
                Type::Path(type_path) => {
                    // Prend le dernier segment du chemin (nom de base du type)
                    let last_segment = type_path.path.segments.last().unwrap();
                    let ident = &last_segment.ident;

                    // Si le type a des arguments (ex. `Type<Arg1, Arg2>`), on les simplifie également
                    if let syn::PathArguments::AngleBracketed(args) = &last_segment.arguments {
                        let args_str = args
                            .args
                            .iter()
                            .map(|arg| match arg {
                                syn::GenericArgument::Type(inner_ty) => simplify_type(inner_ty),
                                _ => arg.to_token_stream().to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("{}<{}>", ident, args_str)
                    } else {
                        ident.to_string()
                    }
                }
                _ => ty.to_token_stream().to_string(),
            }
        }

        simplify_type(&parsed_type)
    }

    fn get_key<Msg: 'static, Client: 'static>(&self) -> (TypeId, TypeId) {
        (TypeId::of::<Msg>(), TypeId::of::<Client>())
    }

    fn get_or_insert_labels<Msg: 'static, Client: 'static>(&mut self, key: &(TypeId, TypeId)) {
        self.labels.entry(*key).or_insert_with(|| {
            [
                KeyValue::new("msg", BusMetrics::simplify_type_name(type_name::<Msg>())),
                KeyValue::new(
                    "client_id",
                    BusMetrics::simplify_type_name(type_name::<Client>()),
                ),
            ]
        });
    }

    pub fn send<Msg: 'static, Client: 'static>(&mut self) {
        let key = self.get_key::<Msg, Client>();
        self.get_or_insert_labels::<Msg, Client>(&key);
        self.send.add(1, self.labels.get(&key).unwrap());
    }

    pub fn receive<Msg: 'static, Client: 'static>(&mut self) {
        let key = self.get_key::<Msg, Client>();
        self.get_or_insert_labels::<Msg, Client>(&key);
        self.receive.add(1, self.labels.get(&key).unwrap());
    }
}
