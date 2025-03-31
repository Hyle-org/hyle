use anyhow::Context;
use http_body_util::BodyExt;
use hyper::{client::conn::http1, Method, Request, Uri};
use hyper_util::rt::TokioIo;
use serde::{de::DeserializeOwned, Serialize};

#[derive(Clone)]
pub struct HttpClient {
    pub url: Uri,
    pub api_key: Option<String>,
}
impl HttpClient {
    pub async fn request<T, R>(
        &self,
        endpoint: &str,
        method: Method,
        content_type: String,
        body: Option<&T>,
        context_msg: &str,
    ) -> anyhow::Result<R>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        // Construire l'URI complète
        let full_url = format!("{}{}", &self.url, endpoint);
        let uri: Uri = full_url.parse().context("URI invalide")?;

        // Établir une connexion TCP
        let authority = uri.authority().context("URI sans autorité")?.clone();
        let host = authority.host().to_string();
        let port = authority.port_u16().unwrap_or(80);
        let addr = format!("{}:{}", host, port);
        let stream = crate::net::TcpStream::connect(addr)
            .await
            .context("Échec de la connexion TCP")?;
        let io = TokioIo::new(stream);

        // Initialiser le client HTTP
        let (mut sender, connection) = http1::handshake(io)
            .await
            .context("Échec de la poignée de main HTTP/1")?;

        // Lancer la tâche pour gérer la connexion
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("Erreur de connexion: {:?}", e);
            }
        });

        // Construire la requête HTTP
        let mut req_builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(hyper::header::CONTENT_TYPE, content_type);

        // Ajouter la clé API si présente
        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("X-API-KEY", key);
        }

        let request = if let Some(b) = body {
            let json_body = serde_json::to_string(b)
                .context("Échec de la sérialisation du corps de la requête")?;

            req_builder
                .body(json_body)
                .context("Échec de la construction de la requête")?
        } else {
            req_builder.body("".to_string())?
        };

        // Envoyer la requête et obtenir la réponse
        let response = sender
            .send_request(request)
            .await
            .context(format!("{}: échec de la requête", context_msg))?;

        // Vérifier le statut HTTP
        if !response.status().is_success() {
            anyhow::bail!(
                "{}: échec avec le statut {}",
                context_msg,
                response.status()
            );
        }

        // Lire et désérialiser le corps de la réponse
        let mut body = response.into_body();

        let mut response: Vec<u8> = vec![];

        // Stream the body, writing each chunk to stdout as we get it
        // (instead of buffering and printing at the end).
        while let Some(next) = body.frame().await {
            let frame = next?;
            if let Some(chunk) = frame.data_ref() {
                response.extend(chunk);
            }
        }

        let result: R = serde_json::from_slice(&response).context(format!(
            "{}: échec de la désérialisation de la réponse",
            context_msg
        ))?;

        Ok(result)
    }

    pub async fn get<R>(&self, endpoint: &str, context_msg: &str) -> anyhow::Result<R>
    where
        R: DeserializeOwned,
    {
        self.request::<String, R>(
            endpoint,
            Method::GET,
            "application/json".to_string(),
            None,
            context_msg,
        )
        .await
    }

    pub async fn post_json<T, R>(
        &self,
        endpoint: &str,
        body: &T,
        context_msg: &str,
    ) -> anyhow::Result<R>
    where
        R: DeserializeOwned,
        T: Serialize,
    {
        self.request::<T, R>(
            endpoint,
            Method::POST,
            "application/json".to_string(),
            Some(body),
            context_msg,
        )
        .await
    }
}
