import { describe, expect, it } from 'vitest'
import {
  buildExecutionOrder,
  executionOrderBytes,
  MANUAL_INTERVENTION_CONTRACT,
  MAX_EXECUTION_ORDER_BYTES,
} from './orderTemplates'

const encoder = new TextEncoder()

describe('executionOrderBytes', () => {
  it('counts the UTF-8 bytes of the order Yard sends, contract included', () => {
    const contractBytes = encoder.encode(MANUAL_INTERVENTION_CONTRACT).length
    expect(executionOrderBytes('  ship it  ')).toBe(
      encoder.encode(buildExecutionOrder('ship it')).length,
    )
    expect(executionOrderBytes('ship it')).toBe(7 + 2 + contractBytes)
    // Each é is two UTF-8 bytes, which a UTF-16 maxLength would miss.
    expect(executionOrderBytes('é'.repeat(10))).toBe(20 + 2 + contractBytes)
  })

  it('places the limit at the server cap for the whole order', () => {
    const contractBytes = encoder.encode(MANUAL_INTERVENTION_CONTRACT).length
    const fits = 'x'.repeat(MAX_EXECUTION_ORDER_BYTES - contractBytes - 2)
    expect(executionOrderBytes(fits)).toBe(MAX_EXECUTION_ORDER_BYTES)
    expect(executionOrderBytes(`${fits}x`)).toBe(MAX_EXECUTION_ORDER_BYTES + 1)
  })
})
