/* PIC16F87XA top-level entry: standard types, status codes, build-time
 * device selection, and the SFR mapping layer. DS39582B is authoritative
 * for every constant; each peripheral header cites its own section.
 * Device (DS39582B §1.0, Table 1-1; non-A 870/871/872/873/874/876/877
 * per DS30529, plus the LF873A voltage variant): 870/871/872/873/873A/
 * LF873A/874/874A/876/876A/877/877A, 2/4/8 KW flash,
 * 128/192/368 B RAM, 64/128/256 B EEPROM, 5/8 ADC channels. */

#ifndef PIC16F87XA_H
#define PIC16F87XA_H

/* standard types. */

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

/* build-time device selection. */

/**
 * @defgroup  PIC16F87XA_Device Device Selection
 * @brief     Select exactly one target device before including any
 *            peripheral header. Defaults to PIC16F877A when none is set.
 * @{
 */
#if !defined(PIC16F870) && !defined(PIC16F871) && !defined(PIC16F872) && !defined(PIC16F873) && !defined(PIC16LF873A) && !defined(PIC16F873A) && \
    !defined(PIC16F874) && !defined(PIC16F874A) && \
    !defined(PIC16F876) && !defined(PIC16F876A) && \
    !defined(PIC16F877) && !defined(PIC16F877A)
#define PIC16F877A   1
#endif

#if defined(PIC16F870) + defined(PIC16F871) + defined(PIC16F872) + defined(PIC16F873) + defined(PIC16LF873A) + defined(PIC16F873A) + defined(PIC16F874) + \
    defined(PIC16F874A) + defined(PIC16F876) + defined(PIC16F876A) + \
    defined(PIC16F877) + defined(PIC16F877A) > 1
#error "Define exactly one of PIC16F870 / PIC16F871 / PIC16F872 / PIC16F873 / PIC16LF873A / PIC16F873A / PIC16F874 / PIC16F874A / PIC16F876 / PIC16F876A / PIC16F877 / PIC16F877A."
#endif

#if   defined(PIC16F870)
  #define PIC16F87XA_FAMILY_FLASH_KW   2
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    0
  #define PIC16F87XA_FAMILY_HAS_CCP2   0
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  128
  #define PIC16F87XA_FAMILY_EEPROM_B   64
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16F870"
#elif defined(PIC16F871)
  #define PIC16F87XA_FAMILY_FLASH_KW   2
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    0
  #define PIC16F87XA_FAMILY_HAS_CCP2   0
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  128
  #define PIC16F87XA_FAMILY_EEPROM_B   64
  #define PIC16F87XA_FAMILY_ADC_CH     8
  #define PIC16F87XA_FAMILY_HAS_PORTD  1
  #define PIC16F87XA_FAMILY_HAS_PORTE  1
  #define PIC16F87XA_FAMILY_HAS_PSP    1
  #define PIC16F87XA_DEVICE_NAME       "PIC16F871"
#elif defined(PIC16F872)
  #define PIC16F87XA_FAMILY_FLASH_KW   2
  #define PIC16F87XA_FAMILY_HAS_USART  0
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   0
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  128
  #define PIC16F87XA_FAMILY_EEPROM_B   64
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16F872"
#elif defined(PIC16F873)
  #define PIC16F87XA_FAMILY_FLASH_KW   4
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  192
  #define PIC16F87XA_FAMILY_EEPROM_B   128
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16F873"
#elif defined(PIC16F873A)
  #define PIC16F87XA_FAMILY_FLASH_KW   4
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_HAS_COMP   1
  #define PIC16F87XA_FAMILY_HAS_VREF   1
  #define PIC16F87XA_FAMILY_RAM_BYTES  192
  #define PIC16F87XA_FAMILY_EEPROM_B   128
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16F873A"
#elif defined(PIC16LF873A)
  #define PIC16F87XA_FAMILY_FLASH_KW   4
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_HAS_COMP   1
  #define PIC16F87XA_FAMILY_HAS_VREF   1
  #define PIC16F87XA_FAMILY_RAM_BYTES  192
  #define PIC16F87XA_FAMILY_EEPROM_B   128
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16LF873A"
#elif defined(PIC16F874)
  #define PIC16F87XA_FAMILY_FLASH_KW   4
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  192
  #define PIC16F87XA_FAMILY_EEPROM_B   128
  #define PIC16F87XA_FAMILY_ADC_CH     8
  #define PIC16F87XA_FAMILY_HAS_PORTD  1
  #define PIC16F87XA_FAMILY_HAS_PORTE  1
  #define PIC16F87XA_FAMILY_HAS_PSP    1
  #define PIC16F87XA_DEVICE_NAME       "PIC16F874"
#elif defined(PIC16F874A)
  #define PIC16F87XA_FAMILY_HAS_COMP   1
  #define PIC16F87XA_FAMILY_HAS_VREF   1
  #define PIC16F87XA_FAMILY_FLASH_KW   4
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_RAM_BYTES  192
  #define PIC16F87XA_FAMILY_EEPROM_B   128
  #define PIC16F87XA_FAMILY_ADC_CH     8
  #define PIC16F87XA_FAMILY_HAS_PORTD  1
  #define PIC16F87XA_FAMILY_HAS_PORTE  1
  #define PIC16F87XA_FAMILY_HAS_PSP    1
  #define PIC16F87XA_DEVICE_NAME       "PIC16F874A"
#elif defined(PIC16F876)
  #define PIC16F87XA_FAMILY_FLASH_KW   8
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  368
  #define PIC16F87XA_FAMILY_EEPROM_B   256
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16F876"
#elif defined(PIC16F876A)
  #define PIC16F87XA_FAMILY_HAS_COMP   1
  #define PIC16F87XA_FAMILY_HAS_VREF   1
  #define PIC16F87XA_FAMILY_FLASH_KW   8
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_RAM_BYTES  368
  #define PIC16F87XA_FAMILY_EEPROM_B   256
  #define PIC16F87XA_FAMILY_ADC_CH     5
  #define PIC16F87XA_FAMILY_HAS_PORTD  0
  #define PIC16F87XA_FAMILY_HAS_PORTE  0
  #define PIC16F87XA_FAMILY_HAS_PSP    0
  #define PIC16F87XA_DEVICE_NAME       "PIC16F876A"
#elif defined(PIC16F877)
  #define PIC16F87XA_FAMILY_FLASH_KW   8
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_HAS_COMP   0
  #define PIC16F87XA_FAMILY_HAS_VREF   0
  #define PIC16F87XA_FAMILY_RAM_BYTES  368
  #define PIC16F87XA_FAMILY_EEPROM_B   256
  #define PIC16F87XA_FAMILY_ADC_CH     8
  #define PIC16F87XA_FAMILY_HAS_PORTD  1
  #define PIC16F87XA_FAMILY_HAS_PORTE  1
  #define PIC16F87XA_FAMILY_HAS_PSP    1
  #define PIC16F87XA_DEVICE_NAME       "PIC16F877"
#else  /* PIC16F877A */
  #define PIC16F87XA_FAMILY_HAS_COMP   1
  #define PIC16F87XA_FAMILY_HAS_VREF   1
  #define PIC16F87XA_FAMILY_FLASH_KW   8
  #define PIC16F87XA_FAMILY_HAS_USART  1
  #define PIC16F87XA_FAMILY_HAS_SSP    1
  #define PIC16F87XA_FAMILY_HAS_CCP2   1
  #define PIC16F87XA_FAMILY_RAM_BYTES  368
  #define PIC16F87XA_FAMILY_EEPROM_B   256
  #define PIC16F87XA_FAMILY_ADC_CH     8
  #define PIC16F87XA_FAMILY_HAS_PORTD  1
  #define PIC16F87XA_FAMILY_HAS_PORTE  1
  #define PIC16F87XA_FAMILY_HAS_PSP    1
  #define PIC16F87XA_DEVICE_NAME       "PIC16F877A"
#endif
/** @} */

/* Family-neutral capability aliases (epic-common contract): exposed
 * under family-neutral names too, so family-agnostic consumers (the
 * task manager) can scale without referencing a family-specific macro.
 * `pic18fxx5x.h` defines the same names to its own values. */
#define EPIC_FAMILY_RAM_BYTES   PIC16F87XA_FAMILY_RAM_BYTES

/* EPIC_StatusTypeDef/EPIC_OK/... and the EPIC_BIT* macros are
 * architecture-blind, so they live in the shared layer; pulled in here
 * so one `#include "pic16f87xa.h"` gives every consumer the same
 * status/bit vocabulary. */
#include "core/hal_status.h"

/* platform: SFR mapping + weak attribute. */
/**
 * @defgroup PIC16F87XA_SFR Special Function Register mapping
 * @brief   How every SFR is stored and how the weak attribute is spelled.
 *
 * Same source reads `epic_sfr_read8()`/`EPIC_REG8()` on both builds;
 * the build's include path picks `include/host/...` (memory-backed) or
 * `include/target/...` (direct volatile deref), not `#ifdef`.
 * @{
 */
#include "pic16f87xa_platform.h"
/** @} */

#endif /* PIC16F87XA_H */
