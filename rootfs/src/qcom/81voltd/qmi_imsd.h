/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Copyright (c) 2024, Richard Acayan. All rights reserved. */
#ifndef __QMI_IMSD_H__
#define __QMI_IMSD_H__

#include <stdint.h>
#include <stdbool.h>

#include "libqrtr.h"

struct imsd_wds_spec {
	uint32_t apn_len;
	char apn[256];
	uint32_t unkfield_0_5;
	uint32_t profiles_select;
	uint32_t ip_family;
	uint16_t profile_idx_3gpp;
	uint16_t profile_idx_3gpp2;
};

struct imsd_address {
	uint32_t family;
	uint32_t addr_len;
	char addr[256];
};

struct imsd_op_res {
	uint16_t err_status;
	uint16_t err_code;
};

struct imsd_start_connection_req {
	struct imsd_wds_spec conn_params;
	uint32_t connection;
	bool echo_sub_valid;	/* TLV 0x12: dual-SIM, echo in CHANGED */
	uint32_t echo_sub;
	uint32_t subscription;	/* TLV 0x13 */
};

struct imsd_start_connection_resp {
	struct imsd_op_res res;
	uint8_t connection;
	uint32_t orig_conn_id;
	uint32_t subscription;
};

struct imsd_connection_changed_ind {
	struct imsd_op_res res;
	uint8_t connection;
	uint32_t orig_conn_id;
	bool ip_addr_valid;
	struct imsd_address ip_addr;
	uint32_t subscription;
};

struct imsd_stop_connection_req {
	uint8_t connection;
	uint32_t unkecho_01;
};

struct imsd_stop_connection_resp {
	struct imsd_op_res res;
	uint8_t unkfield_FA;
	uint32_t unkecho_01;
};

struct imsd_no_op_req {
	bool data_valid;
	uint32_t data_len;
	uint8_t data[255];
};

struct imsd_no_op_resp {
	struct imsd_op_res res;
};

extern struct qmi_elem_info imsd_start_connection_req_ei[];
extern struct qmi_elem_info imsd_start_connection_resp_ei[];
extern struct qmi_elem_info imsd_connection_changed_ind_ei[];
extern struct qmi_elem_info imsd_stop_connection_req_ei[];
extern struct qmi_elem_info imsd_stop_connection_resp_ei[];
extern struct qmi_elem_info imsd_no_op_req_ei[];
extern struct qmi_elem_info imsd_no_op_resp_ei[];

#endif
