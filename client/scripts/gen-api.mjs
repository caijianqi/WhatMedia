/**
 * 从服务端导出的 OpenAPI 契约生成前端类型。
 *
 * 这个脚本**不做**契约生成，只做类型生成 —— 契约本身由
 * `cargo run --features openapi -p whatmedia -- openapi` 产出并签入
 * `docs/openapi/openapi.json`。之所以分开：前端工具链不该依赖 Rust 工具链，
 * 而"契约与代码是否同步"由 `make openapi-check` 单独守。
 *
 * 产物 `src/api/schema.d.ts` 是**生成物**，不要手改。
 */
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { existsSync } from 'node:fs'

const here = dirname(fileURLToPath(import.meta.url))
const client = resolve(here, '..')
const root = resolve(client, '..')

const spec = resolve(root, 'docs/openapi/openapi.json')
const out = resolve(client, 'src/api/schema.d.ts')

if (!existsSync(spec)) {
  console.error(
    `缺少契约文件：${spec}\n` +
      '先跑：cargo run --features openapi -p whatmedia -- openapi',
  )
  process.exit(1)
}

const bin = resolve(client, 'node_modules/openapi-typescript/bin/cli.js')
execFileSync(process.execPath, [bin, spec, '-o', out], { stdio: 'inherit' })
console.log(`已生成 ${out}`)
