use anyhow::Result;
use axum::body::Body;
use axum::extract::State;
use axum::response::IntoResponse;

use axum::{
    routing::{get, post},
    Json, Router,
};
use hyper::{http::{Request, header::{ACCEPT, ACCEPT_ENCODING, 
                                     AUTHORIZATION, CONTENT_TYPE, ORIGIN}}, 
           StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct AppState {
    pool: sqlx::PgPool,
}


#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct Todo {
    id: i32,
    title: String,
    completed: bool,
}


#[derive(Debug, Serialize, Deserialize)]
struct CreateTodo {
    title: String,
    completed: bool,
}

impl AppState {
    async fn new() -> Result<Self> {
        tracing::info!(commit_sha = std::env::var("COMMIT_SHA").unwrap_or(String::from("development")), "initializing...");
        tracing::info!(database_url = std::env::var("DATABASE_URL")?, "Connecting to database");
        let pool = PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(30))
            .max_connections(5)
            .connect(&std::env::var("DATABASE_URL")?)
            .await?;
        tracing::info!("Running migrations");
        sqlx::migrate!("./migrations").run(&pool).await?;
        tracing::info!("Migration complete");
        Ok(Self { pool })
    }
}


async fn create_todo(
    State(state): State<AppState>,
    Json(payload): Json<CreateTodo>,
) -> impl IntoResponse {
    let local_pool = state.pool.clone();
    let todo = sqlx::query_as::<_,Todo>("insert into todos (title, completed) values ($1, $2) returning *")
        .bind(&payload.title)
        .bind(&payload.completed)
        .fetch_one(&local_pool).await;
     match todo {
         Ok(todo) => (StatusCode::OK, Json(todo)).into_response(),
         Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": err.to_string()}))).into_response(),
     }
}

async fn get_todos(State(state): State<AppState>) -> impl IntoResponse {
    let local_pool = state.pool.clone();
    let todos = sqlx::query_as::<_,Todo>("select * from todos")
                     .fetch_all(&local_pool).await;
    match todos {
        Ok(todo) => (StatusCode::OK, Json(todo)).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": err.to_string()}))).into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::INFO)
        .json()
        .init();

    let state = AppState::new().await?;

    // Trace every request
    let trace_layer =
        TraceLayer::new_for_http().on_request(|_: &Request<Body>, _span: &tracing::Span| {
            tracing::info!(message = "begin request")
        });

    // Set up CORS
    let cors_layer = CorsLayer::new()
        .allow_headers(vec![
            ACCEPT,
            ACCEPT_ENCODING,
            AUTHORIZATION,
            CONTENT_TYPE,
            ORIGIN,
        ])
        .allow_methods(tower_http::cors::Any)
        .allow_origin(tower_http::cors::Any);

    // Wrap an `axum::Router` with our state, CORS, Tracing, & Compression layers
    let app = Router::new()
        .route("/", post(create_todo))
        .route("/", get(get_todos))
        .layer(cors_layer)
        .layer(trace_layer)
        .layer(CompressionLayer::new().gzip(true).deflate(true))
        .with_state(state);

    let listener = TcpListener::bind("0..0.0.0:3000").await?;
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();

    // If we compile in release mode, use the Lambda Runtime
    Ok(())
}
