use yew::prelude::*;

use crate::icons;

#[function_component(Footer)]
pub fn footer() -> Html {
    html! {
        <footer class="footer">
            <div class="container footer-inner">
                <div class="footer-brand">
                    { icons::logo() }
                    <div>
                        <p class="footer-name">{ "Qdrant_Rust" }</p>
                        <p class="footer-sub">{ "向量工作台 · Vector Studio" }</p>
                    </div>
                </div>
                <nav class="footer-links" aria-label="页脚导航">
                    <a href="#overview">{ "概览" }</a>
                    <a href="#pipeline">{ "入库流程" }</a>
                    <a href="#config">{ "模型接入" }</a>
                    <a href="#browser">{ "数据浏览器" }</a>
                </nav>
                <div class="footer-meta">
                    <span>{ "Rust · Axum · Yew · Qdrant" }</span>
                    <span>{ "密钥仅驻留本机内存" }</span>
                    <span>{ "© 2026 Qdrant_Rust" }</span>
                </div>
            </div>
        </footer>
    }
}
