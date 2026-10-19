import { describe, expect, it } from 'vitest'
import { toError } from './invoke'

/**
 * Tauri command 失败时, Rust `AppError` 的自定义 `Serialize` 把错误写成
 * **字符串**(`serializer.serialize_str(&self.to_string())`), 所以前端 catch
 * 到的是 string 而不是 Error。
 *
 * 曾经的写法 `(error as Error).message || '备份失败'` 在这种情况下恒为
 * `undefined`, 于是界面永远只显示兜底文案 —— issue #15 里"备份失败"四个字
 * 背后真正的原因(条目超限 / 路径非法 / 网络错误)一个字都没传到用户眼前。
 */
describe('toError', () => {
  it('keeps the original message when Tauri rejects with a string', () => {
    const error = toError('备份条目 reader-rust.json 超过 64 MB')

    expect(error).toBeInstanceOf(Error)
    expect(error.message).toBe('备份条目 reader-rust.json 超过 64 MB')
  })

  it('returns the same Error instance unchanged', () => {
    const original = new Error('无法连接 WebDAV 服务器')

    expect(toError(original)).toBe(original)
  })

  it('falls back when the rejection carries no usable text', () => {
    // undefined / 空串 / 空对象: 都没有可展示的信息
    expect(toError(undefined, '备份失败').message).toBe('备份失败')
    expect(toError('', '备份失败').message).toBe('备份失败')
    expect(toError('   ', '备份失败').message).toBe('备份失败')
    expect(toError({}, '备份失败').message).toBe('备份失败')
  })

  it('reads a message off a non-Error rejection object', () => {
    expect(toError({ message: '网络超时' }).message).toBe('网络超时')
  })

  it('uses a generic default when no fallback is supplied', () => {
    expect(toError(null).message).toBe('请求失败')
  })
})
