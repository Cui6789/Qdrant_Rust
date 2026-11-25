use yew::prelude::*;

use crate::icons;

#[function_component(Features)]
pub fn features() -> Html {
    html! {
        <section class="section features-section">
            <div class="container">
                <div class="features-intro reveal">
                    <p class="eyebrow">{ "Built for observability" }</p>
                    <h2>{ "为观察而生的知识库。" }</h2>
                    <p class="section-sub">
                        { "Qdrant_Rust 原本是一条安静的后端管道。这个工作台把它的每一步搬到台前。" }
                    </p>
                </div>
                <div class="feature-grid">
                    <article class="feature-card reveal">
                        <span class="feature-icon">{ icons::activity() }</span>
                        <h3>{ "实时读取" }</h3>
                        <p>{ "每 3.5 秒直接读取 Qdrant payload，按文档聚合块、字符、标签与写入时间。" }</p>
                        <span class="feature-note">{ "gRPC · 实时" }</span>
                    </article>
                    <article class="feature-card feature-card-dark reveal">
                        <span class="feature-icon">{ icons::spark() }</span>
                        <h3>{ "任意嵌入" }</h3>
                        <p>{ "接入任意 OpenAI 兼容端点，自动探测真实维度；密钥只驻留本机内存。" }</p>
                        <span class="feature-note">{ "OpenAI · Ollama · vLLM" }</span>
                    </article>
                    <article class="feature-card reveal">
                        <span class="feature-icon">{ icons::pulse() }</span>
                        <h3>{ "完整链路" }</h3>
                        <p>{ "解析、切块、嵌入、写入与清理都有真实事件、耗时与首条向量预览。" }</p>
                        <span class="feature-note">{ "端到端可视" }</span>
                    </article>
                </div>
            </div>
        </section>
    }
}
