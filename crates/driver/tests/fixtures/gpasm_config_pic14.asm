	list p=16f877a
	radix hex
#include <p16f877a.inc>
	__CONFIG _CP_OFF & _WDT_OFF & _BODEN_ON & _PWRTE_ON & _LVP_OFF & _CPD_OFF & _WRT_OFF & _DEBUG_OFF & _XT_OSC
	org 0
	nop
	end
